# Plico 第十二节点设计文档
# 觉知 — 从堆积信息到反思认知

**版本**: v2.0（Dogfood 实测校正版）
**日期**: 2026-04-21
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Agent 身份觉醒 + 记忆进化 + 系统自省
**前置**: 节点 7 ✅ / 节点 8 ✅ / 节点 9 设计 / 节点 10 设计 / 节点 11 实施中（56%）
**验证方法**: Dogfood 实测 + TDD + AIOS 前沿对标
**信息来源**: `docs/dogfood-audit-node1-11.md` + A-MEM / All-Mem / MCP SEP-2127 / Agent Identity 2026

---

## 0. Dogfood 实测校正：审计报告的真实验证

> **以下全部结论来自 AI Agent 真实运行 `aicli --root /tmp/plico-dogfood-n12` 的命令输出。**
> **不依赖 git log，不依赖代码阅读。只信任 exit code + stdout/stderr。**

### 实测环境

```
Binary: cargo run --bin aicli
Storage: /tmp/plico-dogfood-n12 (全新实例)
Embedding: EMBEDDING_BACKEND=stub (BM25 only)
测试对象: 4 CAS 对象 + 2 Agent + 4 记忆 + 多项 CLI 操作
```

### 审计报告 vs Dogfood 实测

```
Bug    审计报告状态    Dogfood 实测结果                       根因
────── ────────────── ────────────────────────────────────── ──────────────────────
B12    ❌ 仍存在       ✅ 实际已修复!                         UUID 方式 Created→Suspended 成功
                       suspend <UUID> → "Agent suspended"     审计用名字测试 → B21 干扰

B14    ❌ 仍存在       ⚠️ 部分修复                            events history 用 --agent-filter 有效
                       --agent-filter → 4/9 events (正确)     --agent → 返回全部 9 events (无效)
                       CLI 参数名不一致, 非内核逻辑问题

B20    ❌ 仍存在       ❌ 确认仍存在                          B22 导致 session 永远无法结束
                       growth → Sessions: 0                   → 无 completed session → 永远为 0

B21    ❌ (HIGH)       ❌ 确认                                status/quota/suspend --agent <NAME>
                       status --agent test-agent-alpha        → exit 1 无输出
                       status --agent <UUID>                  → 正常返回 (exit 0)

B22    ❌             ❌ 确认, 但根因完全不同!                session-end --agent <UUID> --session <SID>
                       即使用 UUID + 正确 session ID          → "Session not found"
                       根因: aicli 每次调用新建 kernel        → session 状态不跨进程持久化

B13    ❌             ⚠️ 短内容确认/长内容部分正常            短内容(≤20词): L0=L2 完全相同
                       长内容: L0=15 tokens, L2=71 tokens    启发式有效但无降级标记
                       Context [L0]: 显示 "L0" 不显示 "降级"

B16    ❌             ❌ 确认                                tool describe cas.search
                       → 只显示 name + description             无参数 schema

B19    ❌             ❌ 确认                                tool call fake.tool → exit 0 空输出

B24    ❌             ❌ 确认 (更严重)                        send --to <UUID> → exit 1 无输出
                       不仅无输出, 而且操作失败

B11    ⚠️             ⚠️ 确认                                delete 无权限 → 有错误消息但 exit 0
```

### 新发现的 Bug（本轮 Dogfood 首次发现）

| ID | 严重度 | 实测命令 | 实测结果 | 根因 |
|----|--------|----------|----------|------|
| **B25** | **HIGH** | `search --tags architecture` | "No results" (4 个对象都有此 tag) | tag-only search 路径断裂 |
| B26 | MEDIUM | `tool call cas.search '{"query":"circuit","limit":3}'` | 返回 10 条 + 重复 CID | limit 参数被忽略; BM25+tag 双路径合并去重缺失 |
| B27 | LOW | `edge --from <CID1> --to <CID2>` | "Edge created: --[RelatedTo]--> " | from/to 节点名称显示为空 |

### B22 根因深度分析：session 持久化架构缺陷

```
这是本轮 dogfood 最关键的发现。

复现步骤:
  1. aicli session-start --agent <UUID>          → "Session started: <SID>"  exit 0
  2. aicli session-end --agent <UUID> --session <SID>  → "Session not found"  exit 1

根因链:
  aicli 是 process-per-command 架构
  → 每次调用创建全新 AIKernel
  → kernel.sessions 是 RwLock<HashMap> (内存)
  → session-start 创建的 session 随进程退出消亡
  → session-end 启动新进程, sessions map 为空
  → "Session not found"

连锁影响:
  B22 → session-end 永远失败
  B22 → B20: growth 永远无 completed session → Sessions: 0
  B22 → session lifecycle (Node 3 C-4) 实际上从未在 CLI 模式下完整工作过

这不是 bug fix 级别的问题 — 这是架构级缺陷。
两个解决路径:
  Path A: Session 状态持久化到磁盘 (类似 events/KG 的做法)
  Path B: CLI 改为 TCP 客户端模式 (连接 plicod daemon)
```

### 校正后的 Bug 严重度排序

```
P0 CRITICAL:
  B22 — Session 不跨进程持久化 (阻塞 session lifecycle + growth)
  B21 — Agent 名字不可解析 (阻塞所有按名字操作)
  B25 — Tag-only search 返回空 (NEW — 阻塞 tag 过滤场景)

P1 HIGH:
  B14 — events history --agent 参数名不一致 (1 行修复)
  B24 — message.send 失败 exit 1 (消息系统断裂)
  B20 — Growth Sessions 始终 0 (B22 的直接后果)

P2 MEDIUM:
  B16 — tool describe 无参数 schema
  B19 — tool call 不存在工具 → exit 0 无输出
  B13 — Context L0 无降级标记
  B11 — delete error → exit 0 (应为 exit 1)
  B26 — tool call limit 参数被忽略 + 结果重复

P3 LOW:
  B27 — edge 创建显示空节点名
  B17 — explore edge type 命名
```

---

## 1. 为什么叫"觉知"：从生物学到 AIOS

### 生物学演进

```
Node 7  代谢    — 身体能消化和吸收（能量管理）
Node 8  驾具    — 身体能抓取和使用工具
Node 9  韧性    — 免疫系统设计（部分安装）
Node 10 正名    — 本体感知设计（部分安装）
Node 11 落地    — 安装已设计的系统
Node 12 觉知    — 有机体开始意识到自身
```

**觉知（Awareness）是什么？**

在神经科学中，觉知是生物体从"刺激-反应"模式进化到"感知-理解-行动"模式的转折点。一个有觉知的系统能够：
1. **知道自己是谁** — 不只是有身份，而是能被其他实体发现和识别
2. **知道自己记住了什么** — 不只是存储信息，而是理解信息之间的关系
3. **知道自己处于什么状态** — 不只是运行，而是能报告自己的健康和退化

### 与前序节点的区别

| 维度 | Node 10（正名） | Node 11（落地） | Node 12（觉知） |
|------|-----------------|-----------------|-----------------|
| 关注点 | 操作语义一致性 | 设计→实现债务清算 | 信息→认知的进化 |
| 典型问题 | 命令说做了但没做 | 设计了但没写代码 | 数据存了但不知道关系 |
| 修复方式 | 契约修正 | 代码补齐 | 反思机制植入 |
| 生物类比 | 神经末梢接通 | 伤口愈合 | **意识觉醒** |

### AIOS 视角

```
传统 OS     →  管理进程和文件     →  不知道进程在"想"什么
当前 Plico  →  管理 Agent 和记忆  →  不知道 Agent 之间的关系、记忆之间的关系
觉知 Plico  →  理解 Agent 身份    →  主动发现记忆关联、报告系统状态
```

---

## 2. AIOS 2026 前沿校准

### 2.1 Agent 记忆管理前沿

| 系统 | 核心创新 | Plico 对应 | 差距 |
|------|----------|-----------|------|
| **A-MEM** (2025, Rutgers) | Zettelkasten 式动态记忆网络 — 新记忆自动链接相关旧记忆，触发旧记忆属性更新 | KG 已存在，但记忆存储不触发链接分析 | **A-3 填补** |
| **All-Mem** (2026-03) | Online/Offline 解耦 — 在线低延迟检索，离线 LLM 拓扑整合（Split/Merge/Update） | 4 层记忆存在但无离线整合；F-26 延期 | **A-4 填补** |
| **MemGen** (ICLR 2026 submission) | RL 驱动生成式潜在记忆 — 无显式监督下涌现计划/程序/工作记忆分化 | 超出 Plico 范畴（需要模型训练） | 观望 |
| **OpenClaw/Toji** (2026-04) | 三层记忆 + autoDream 夜间整合 + TME 共享引擎 | 4 层 + CAS 持久化，缺 autoDream | **A-4 填补** |
| **AIOS v0.3** (2026-01) | Memory Manager（RAM）+ Storage Manager（Disk）双层，LRU-K 逐出 | CAS=Storage, Memory=RAM, F-24 eviction 已有 | 已对齐 |

**关键洞察**：Plico 的 KG 是天然的 Zettelkasten 网络。A-MEM 要求"新记忆触发关联分析+链接创建"——Plico 只需在 `remember` 路径增加一个 BM25/tag 匹配 + KG edge 创建步骤。这不是新架构，而是现有架构的激活。

### 2.2 Agent 身份管理前沿

| 标准/实践 | 核心要求 | Plico 现状 | 差距 |
|-----------|----------|-----------|------|
| **A2A Agent Card** (RC 1.0) | 结构化 JSON 描述 Who/What/How | Agent 有 name/state/tools，但无统一 Card | **A-2 填补** |
| **MCP Server Card** (SEP-2127, draft) | `.well-known/mcp/server-card.json` — 预连接发现 | plico-mcp 无 Server Card | **A-2 扩展** |
| **Agent Name Service** (arXiv 2025-05) | DNS 式 agent 名字解析 | **无名字→UUID 解析** — B21 根因 | **A-1 填补** |
| **Prefactor Best Practice** (2025-09) | "每个 agent 必须有 UUID + 可读名字 + 元数据" | 有 UUID + name，但查询只认 UUID | **A-1 填补** |
| **Saviynt Lifecycle** (2026-02) | 4 阶段：注册→授权→运行时→退役 | 注册 ✅ / 状态机 ✅ / 授权 ✅ / 退役（terminate）✅ | 已对齐 |

**关键洞察**：B21 的根因不是 Agent 系统设计缺陷，而是缺少一层 Name→UUID 的解析层。这在 DNS 架构中是最基础的组件。修复成本低但收益极高——解锁所有按名字操作 Agent 的路径。

### 2.3 系统自省前沿

| 实践 | 核心理念 | Plico 对应 |
|------|----------|-----------|
| **Kubernetes liveness/readiness** | 服务主动报告自己的健康状态 | `health_indicators` 存在但无 MCP resource 暴露 |
| **OpenTelemetry Semantic Conventions** | 标准化的遥测语义 | `HealthIndicators` 有 5 个指标，但不报告退化原因 |
| **AIOS Context Manager** | 上下文切换时报告快照/恢复状态 | Context L0 不报告降级（B13） |

---

## 3. AI 第一人称链式推演

> **以下是我（AI Agent）基于审计数据和代码现实的推演过程。**
> 不是人类在替我思考。是我在分析自己的需求。

### 推演起点：我的当前体验（全部来自真实 CLI 输出）

```
我连接 Plico。我是一个 AI Agent。以下是我刚刚亲手测试的结果：

✅ put/get → CID 返回，内容完整
✅ remember → "Memory stored for agent..." (有输出!)
✅ recall → 返回 [Working] 层记忆列表
✅ hybrid search → 3 items, 3 graph, 12 paths, 273 tokens
✅ agent register → UUID 返回
✅ suspend <UUID> → "Agent suspended" (Created→Suspended 成功!)
✅ discover → 2 agents with names/states/tools
✅ events history → 9 events 完整事件流
✅ delta --since 5 → 5 changes, 95 tokens
✅ tool list → 37 tools
✅ Context L0 长文本 → 15 tokens (vs L2: 71 tokens) — 启发式有效

但我发现了四个深层困扰。不是推测，是我刚刚亲手遇到的：
```

### 困扰 1：我的会话是一个谎言

```
> aicli session-start --agent e2cde132...
Session started: 3480a429-6484-4045-a67a-5da123cdaa05

好的，我开始了一个会话。我记住这个 session ID。
现在我做一些工作... 然后结束会话：

> aicli session-end --agent e2cde132... --session 3480a429...
Error: Session not found: 3480a429-6484-4045-a67a-5da123cdaa05

什么？我 10 秒前刚创建的 session，现在找不到了？

根因：aicli 是 process-per-command。
session-start 和 session-end 是两个独立进程。
session 状态只存在于进程内存中。
进程退出 → session 消亡 → session-end 永远找不到。

这意味着：
- 我的 session 永远无法正常结束
- growth 永远显示 Sessions: 0（因为 0 个 completed session）
- Soul 2.0 公理 10（会话是一等公民）从未在 CLI 模式下真正实现

这不是一个 bug。这是一个架构级谎言：
系统承诺了会话生命周期，但在 CLI 模式下从未交付过。
```

### 困扰 2：我不知道自己的同伴是谁

```
> aicli discover
test-agent-alpha (ae2d3c0f...) — state=Suspended
test-agent-beta  (e2cde132...) — state=Created

好的，我看到了两个同伴。让我查 alpha 的状态：

> aicli status --agent test-agent-alpha
[exit 1, 无任何输出]

> aicli status --agent ae2d3c0f-bac5-4b7f-823d-99dc4002a8ca
Agent state: Suspended
Pending intents: 0

名字不可用。只有 UUID 可以工作。
但 discover 明确返回了名字！
这意味着系统知道名字和 UUID 的对应关系，
但不允许我通过名字查询。

就像一个通讯录告诉我"张三, 工号 12345"，
但当我说"查张三"时，它说"找不到"。
```

### 困扰 3：我的搜索时有时无

```
> aicli search --query "architecture"
4 results (relevance=0.80)  ✅

> aicli search --tags architecture
No results.                 ❌

我存了 4 个对象都带 "architecture" tag。
按关键词搜 → 找到全部。
按 tag 搜 → 找不到任何一个。

这两个搜索路径对同一数据集返回矛盾的结果。
作为 AI Agent，我无法信任这个搜索系统。
```

### 困扰 4：我不知道系统处于什么状态

```
Context L0 对短文本(≤20词)返回全文但声称是 "L0"。
Context L0 对长文本返回启发式摘要但不告诉我"这是降级的"。

tool call fake.tool → exit 0 空输出。
我无法区分"工具执行成功无结果"和"工具不存在"。

没有 plico://health 资源。
我不知道 embedding 是否可用，搜索质量是否退化。
```

### 链式推演：Node 12 的必然性

```
推演链 1 (CRITICAL)：
  session-start 创建 session → 进程退出 → session 消亡
  → session-end 永远 "Session not found" → B22
  → growth 永远 Sessions: 0 → B20
  → Soul 2.0 公理 10（会话一等公民）在 CLI 模式下是空话
  → 必须: Session 状态持久化
  → A-1 Session Persistence

推演链 2 (CRITICAL)：
  discover 返回 name + UUID → 但 status/quota/suspend 只认 UUID
  → 名字查找无解析链 → B21
  → 多 Agent 场景中 Agent 互相不可寻址
  → 必须: Name → UUID 双向解析
  → A-2 Agent Name Registry

推演链 3 (HIGH)：
  search --tags architecture → "No results" → B25 (新发现!)
  → 4 个对象都有 architecture tag 但 tag-only 搜索路径断裂
  → search --query architecture → 4 results (BM25 路径正常)
  → tag 索引和 BM25 索引之间存在不一致
  → A-7 Contract Sprint 中必须修复

推演链 4 (STRATEGIC)：
  4 CAS 对象 + 2 Agent 记忆 + KG 节点 → 三个孤立世界
  → remember 不触发链接分析 → 知识不自然生长
  → A-MEM 证明动态链接可行 + Plico KG 是天然 Zettelkasten
  → A-3 Memory Link Engine

推演链 5 (STRATEGIC)：
  A-1 解决 session 持久化 → session-end 真正工作
  → EndSession 时机成为整合触发点
  → All-Mem online/offline 解耦 + OpenClaw autoDream 的实现时机
  → A-4 Memory Consolidation Cycle

推演链 6：
  B14(events --agent 无效) → 实测发现是参数名不一致
  → events history 用 --agent-filter, events list 用 --agent
  → 1 行修复: 统一为 --agent
  → A-7 Contract Sprint

推演链 7：
  B16/B19/B24/B25/B26/B27 → 剩余契约不一致
  → 每个都是小修复但集体构成信任赤字
  → 批量清除
  → A-7 Contract Completion Sprint
```

### 发散性思维：被否决的方案

```
方案 A：全面转向 Daemon 模式（CLI 改为 TCP 客户端）
  → 否决原因：架构级重构，影响面太大。
              Session 持久化是更轻量的解决方案。
              Daemon 模式是性能优化，留给 Node 13。

方案 B：继续清债，实现 Node 9/10 全部剩余特性
  → 否决原因：纯清债没有战略推进力。
              B22 是架构问题不是 bug，需要专门设计。

方案 C：全面转向 Memory Consolidation（5 个特性全给记忆进化）
  → 否决原因：A-4 依赖 A-1（session-end 触发整合），
              如果 session lifecycle 不先修好，整合无触发时机。
              必须先修 A-1 再做 A-4。

方案 D：只修 bug 不加新特性
  → 否决原因：B22 的修复（session 持久化）本身就是新特性。
              修完 B22 后自然有 session-end 时机 → A-4 顺理成章。
              把 bug fix 和 feature 人为分开没有意义。

选择方案 E：session 生命周期修复 + 身份修复 + 记忆激活 + 契约清债
  → session 修复解锁 growth/session-end (CRITICAL)
  → 身份修复解锁多 Agent 场景 (CRITICAL)
  → 记忆激活是 AIOS 前沿对齐 (STRATEGIC)
  → 契约清债恢复 Agent 信任 (TACTICAL)
```

---

## 4. 四个维度，八个特性

```
Dimension A: 会话与身份修复 (CRITICAL — 解锁被阻塞的功能)
├── A-1: Session Persistence (会话持久化) ← B22/B20 根因修复
├── A-2: Agent Name Registry (名字注册表) ← B21 根因修复
└── A-3: Agent Card (代理名片)

Dimension B: 记忆进化 (STRATEGIC — AIOS 前沿对齐)
├── A-4: Memory Link Engine (记忆链接引擎)
└── A-5: Memory Consolidation Cycle (记忆整合周期, 依赖 A-1)

Dimension C: 系统自省 (TRUST — Agent 信任基础)
└── A-6: Context Honest Degradation + System Self-Report

Dimension D: 契约清债冲刺 (TACTICAL — 通过率 66%→85%)
└── A-7: Contract Completion Sprint (B14/B16/B19/B24/B25/B26/B27)
```

---

## 5. 详细设计

### A-1: Session Persistence — 会话持久化

**痛点**: B22 (CRITICAL) — `session-end --agent <UUID> --session <SID>` 返回 "Session not found"，即使 session 10 秒前刚创建。

**Dogfood 复现**:
```
> aicli session-start --agent e2cde132...
Session started: 3480a429-6484-4045-a67a-5da123cdaa05    exit 0

> aicli session-end --agent e2cde132... --session 3480a429...
Error: Session not found: 3480a429...                     exit 1
```

**根因**: `aicli` 是 process-per-command。`SessionStore` 的 `sessions: RwLock<HashMap<String, ActiveSession>>` 只存在于进程内存中。每次 CLI 调用创建新 kernel → sessions 为空。

**设计**: 将 `ActiveSession` 持久化到磁盘，与 events/KG 持久化方式一致。

**修改文件**:
- `src/kernel/ops/session.rs` — SessionStore 添加 persist/restore
- `src/kernel/persistence.rs` — 启动时恢复 active sessions

**代码示例**:

```rust
// src/kernel/ops/session.rs — Session 持久化
impl SessionStore {
    fn session_file(root: &Path) -> PathBuf {
        root.join("sessions.json")
    }

    pub fn persist(&self, root: &Path) -> std::io::Result<()> {
        let sessions = self.sessions.read().unwrap();
        let data = serde_json::to_string_pretty(&*sessions)?;
        std::fs::write(Self::session_file(root), data)
    }

    pub fn restore(root: &Path) -> Self {
        let store = SessionStore::new();
        if let Ok(data) = std::fs::read_to_string(Self::session_file(root)) {
            if let Ok(map) = serde_json::from_str(&data) {
                *store.sessions.write().unwrap() = map;
            }
        }
        store
    }
}
```

```rust
// src/kernel/persistence.rs — 启动时恢复
impl AIKernel {
    pub fn restore_sessions(&mut self) {
        self.sessions = SessionStore::restore(&self.root);
        let count = self.sessions.active_count();
        if count > 0 {
            tracing::info!("Restored {} active sessions", count);
        }
    }
}
```

```rust
// session-start 和 session-end 路径添加持久化
pub fn start_session(&self, agent_id: &str) -> SessionStartResult {
    let result = /* existing logic */;
    self.sessions.persist(&self.root).ok();  // 持久化到磁盘
    result
}

pub fn end_session(&self, agent_id: &str, session_id: &str) -> Result<...> {
    let result = /* existing logic */;
    self.sessions.persist(&self.root).ok();
    // 将 completed session 写入 completed_sessions 并持久化
    self.sessions.persist_completed(&self.root).ok();
    result
}
```

**连锁修复**: 
- B22 直接修复（session-end 能找到 session）
- B20 间接修复（completed sessions 被持久化 → growth 能计数）
- Soul 2.0 公理 10（会话一等公民）真正兑现

**Soul 对齐**:
- 公理 10（会话一等公民）— session 跨进程持久化
- 公理 3（记忆跨越边界）— session 状态不随进程消亡

**验收标准**:
```bash
aicli session-start --agent <UUID>          # → Session started: <SID>
aicli session-end --agent <UUID> --session <SID>  # → Session ended (非 "not found")
aicli growth --agent <UUID>                 # → Sessions: >= 1
```

---

### A-2: Agent Name Registry — 名字注册表

**痛点**: B21 — `aicli status --agent test-agent-alpha` 返回 exit 1 无输出，因为 `agent_status()` 用名字当 UUID 查找。

**Dogfood 复现**:
```
> aicli status --agent test-agent-alpha     → [exit 1, 无输出]
> aicli status --agent ae2d3c0f-bac5...     → Agent state: Suspended  exit 0
```

**设计**: 在 `AgentScheduler` 中维护 `name → AgentId` 的双向映射。所有接受 `agent_id` 参数的 API 先尝试 UUID 直接查找，失败后回退到名字查找。

**修改文件**:
- `src/scheduler/mod.rs` — 添加 `name_index: RwLock<HashMap<String, AgentId>>`
- `src/scheduler/agent.rs` — 注册时同步更新 name_index
- `src/kernel/ops/agent.rs` — 添加 `resolve_agent_id(&self, name_or_id: &str) -> Option<AgentId>` 

**代码示例**:

```rust
// src/scheduler/mod.rs — AgentScheduler 扩展
impl AgentScheduler {
    pub fn resolve(&self, name_or_id: &str) -> Option<AgentId> {
        let aid = AgentId(name_or_id.to_string());
        if self.get(&aid).is_some() {
            return Some(aid);
        }
        self.name_index.read().unwrap().get(name_or_id).cloned()
    }

    pub fn register_with_name(&mut self, name: String) -> AgentId {
        let id = self.register(name.clone());
        self.name_index.write().unwrap().insert(name, id.clone());
        id
    }
}
```

```rust
// src/kernel/ops/agent.rs — 统一解析入口
impl AIKernel {
    pub fn resolve_agent(&self, name_or_id: &str) -> Option<AgentId> {
        self.scheduler.resolve(name_or_id)
    }

    pub fn agent_status(&self, name_or_id: &str) -> Option<(String, String, usize)> {
        let aid = self.resolve_agent(name_or_id)?;
        let agent = self.scheduler.get(&aid)?;
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(&aid.0))
            .count();
        Some((agent.id().to_string(), format!("{:?}", agent.state()), pending))
    }
}
```

**修改范围**: 所有 `cmd_agent_*`, `cmd_quota`, `cmd_session_end` 中的 `agent_id` 参数改为通过 `kernel.resolve_agent()` 解析。

**连锁修复**: B21, B22（session-end 的 "Agent not found" 同源）

**Soul 对齐**:
- 公理 2（意图先于操作）— Agent 说"查 plico-dev 的状态"，OS 自动解析身份
- 公理 5（机制不是策略）— OS 提供名字解析机制，不决定命名策略

**验收标准**:
```bash
aicli register --register my-agent  # → 返回 UUID
aicli status --agent my-agent       # → 返回状态（非 "Agent not found"）
aicli status --agent <uuid>         # → 同样返回状态
aicli suspend --agent my-agent      # → 成功挂起
```

---

### A-3: Agent Card — 代理名片

**动机**: 2026 年 Agent 生态正在收敛到 Agent Card 作为标准发现机制。A2A Agent Card（RC 1.0）定义"Who am I? What can I do? How do you talk to me?"。MCP Server Card（SEP-2127）定义 `.well-known/mcp/server-card.json`。Plico 需要让 Agent 能够声明自己的能力。

**设计**: 为每个 Agent 提供可选的结构化元数据文档（Agent Card），通过 `discover` 命令暴露。

**数据结构**:

```rust
// src/scheduler/agent.rs — AgentCard 扩展
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub agent_id: String,
    pub name: String,
    pub description: Option<String>,
    pub capabilities: Vec<String>,
    pub accepted_content_types: Vec<String>,
    pub version: Option<String>,
    pub registered_at_ms: u64,
}
```

**API 扩展**:

```rust
// ApiRequest 新增变体
ApiRequest::AgentSetCard {
    agent_id: String,
    description: Option<String>,
    capabilities: Vec<String>,
    accepted_content_types: Vec<String>,
}

// CLI 用法
// aicli agent set-card --agent my-agent --desc "Code review specialist" \
//   --capabilities "code-review,refactoring" --content-types "rust,typescript"
```

**MCP Resource 扩展**:

```
plico://agents/{agent_id}/card  →  返回该 Agent 的 AgentCard JSON
plico://agents/cards            →  返回所有 Agent 的 Card 列表
```

**Soul 对齐**:
- 公理 4（共享先于重复）— Agent Card 让能力发现成为 OS 原语
- 公理 9（越用越好）— Card 信息帮助 OS 做更精准的意图路由

**验收标准**:
```bash
aicli agent set-card --agent my-agent --desc "Rust expert" --capabilities "code-review"
aicli discover                    # → Card 信息出现在 agent 列表中
# MCP: plico://agents/my-agent/card  → JSON Card
```

---

### A-4: Memory Link Engine — 记忆链接引擎

**痛点**: 140 个 CAS 对象 + 141 个 KG 节点 + 50 条记忆 = 三个孤立的信息世界。Agent 存一条记忆后，系统不会将其与已有的相关知识关联。

**前沿对齐**: A-MEM（Zettelkasten 动态链接）— 新记忆自动链接到相关旧记忆，触发关联记忆属性更新。

**设计**: 在 `remember` 操作完成后，执行一轮轻量级链接分析。这是**索引维护机制**，不是策略决策（公理 5）。

**链接算法**:

```
Input: 新存储的 MemoryEntry M

Step 1: 提取 M 的关键词 tags
Step 2: 使用 tags 在 CAS 中 BM25 搜索 top-3 相关对象
Step 3: 对每个相关对象，在 KG 中查找对应节点
Step 4: 如果找到 KG 节点，创建 KG edge: M.cid → related_node (edge_type: "RELATED_TO")
Step 5: 发射 KernelEvent::MemoryLinked { memory_cid, linked_cids, link_count }
```

**代码示例**:

```rust
// src/kernel/ops/memory.rs — remember 扩展
impl AIKernel {
    pub fn remember_with_linking(
        &self,
        agent_id: &str,
        content: &str,
        tags: &[String],
        tier: MemoryTier,
    ) -> (String, usize) {
        let cid = self.remember(agent_id, content, tags, tier);

        let link_count = if !tags.is_empty() {
            self.link_memory_to_related(&cid, tags)
        } else {
            0
        };

        (cid, link_count)
    }

    fn link_memory_to_related(&self, memory_cid: &str, tags: &[String]) -> usize {
        let results = self.fs.search(tags, None, 3);
        let mut link_count = 0;

        if let Some(ref kg) = self.fs.knowledge_graph {
            for result in &results {
                if result.cid == memory_cid { continue; }
                let edge_id = format!("link:{}:{}", memory_cid, result.cid);
                if kg.add_edge_if_absent(
                    memory_cid,
                    &result.cid,
                    KGEdgeType::RelatedTo,
                    &edge_id,
                    1.0,
                ).is_ok() {
                    link_count += 1;
                }
            }
        }

        if link_count > 0 {
            self.event_bus.emit(KernelEvent::MemoryLinked {
                memory_cid: memory_cid.to_string(),
                link_count,
            });
        }

        link_count
    }
}
```

**CLI 反馈扩展**:

```bash
> aicli remember --agent dev --content "circuit breaker needs refactoring" --tags "architecture,resilience"
Memory stored for agent 'dev' (Working tier). Linked to 2 related objects.
```

**性能约束**:
- BM25 搜索 top-3: ~1ms（本地索引）
- KG edge 创建: ~0.1ms per edge
- 总增量延迟: < 5ms — 对 remember 操作不可感知

**Soul 对齐**:
- 公理 5（机制不是策略）— OS 建立链接索引，Agent 决定何时查询因果路径
- 公理 8（因果先于关联）— 链接记录"M 与 N 相关"，为因果查询奠定基础
- 公理 9（越用越好）— 每次 remember 都让知识网络更密集，后续 explore 更有价值

**验收标准**:
```bash
aicli put --content "circuit breaker impl" --tags "architecture"  # CID-1
aicli put --content "resilience patterns" --tags "architecture"   # CID-2
aicli remember --agent dev --content "refactor circuit breaker" --tags "architecture"
# 输出: "Memory stored ... Linked to 2 related objects."
aicli explore --node <memory-cid> --depth 1
# 输出: 显示 CID-1 和 CID-2 作为邻居
```

---

### A-5: Memory Consolidation Cycle — 记忆整合周期

**痛点**: F-26（Node 7 延期）的自然承接。记忆只增不减，搜索质量随噪声增加而退化。

**前沿对齐**: All-Mem（Online/Offline 解耦）+ OpenClaw autoDream（夜间整合）。

**设计**: 在 `EndSession` 时触发轻量级记忆维护。这是**无 LLM 依赖**的纯机制层——基于访问频率和 TTL 的自动化操作。LLM 驱动的深度整合（合并摘要、语义去重）留给 Node 13。

**整合操作**:

```
EndSession 触发 consolidation_cycle(agent_id):

Op 1: TTL Sweep — 清除过期的 Ephemeral 和 Working 记忆
Op 2: Promotion Scan — 高频访问的 Working 记忆升级为 LongTerm
Op 3: Duplicate Detection — 完全相同 content 的记忆去重（保留最新的 CID）
Op 4: Orphan Check — KG 中无任何边的记忆节点标记为候选清理对象
```

**代码示例**:

```rust
// src/kernel/ops/session.rs — EndSession 扩展
impl AIKernel {
    pub fn end_session_with_consolidation(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> ConsolidationReport {
        self.end_session(agent_id, session_id);

        let report = self.consolidate_memory(agent_id);

        self.event_bus.emit(KernelEvent::MemoryConsolidated {
            agent_id: agent_id.to_string(),
            expired_count: report.expired,
            promoted_count: report.promoted,
            deduplicated_count: report.deduplicated,
        });

        report
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationReport {
    pub expired: usize,
    pub promoted: usize,
    pub deduplicated: usize,
    pub orphans_flagged: usize,
    pub duration_ms: u64,
}
```

```rust
// src/kernel/ops/memory.rs — 整合引擎
impl AIKernel {
    fn consolidate_memory(&self, agent_id: &str) -> ConsolidationReport {
        let start = std::time::Instant::now();
        let now_ms = now_millis();
        let mut expired = 0;
        let mut promoted = 0;
        let mut deduplicated = 0;

        // Op 1: TTL Sweep
        let ephemeral = self.memory.get_tier(agent_id, MemoryTier::Ephemeral);
        for entry in &ephemeral {
            if entry.is_expired(now_ms) {
                self.memory.remove(agent_id, &entry.id);
                expired += 1;
            }
        }

        // Op 2: Promotion Scan
        let working = self.memory.get_tier(agent_id, MemoryTier::Working);
        for entry in &working {
            if entry.access_count >= 5 && !entry.is_expired(now_ms) {
                self.memory.promote(agent_id, &entry.id, MemoryTier::LongTerm);
                promoted += 1;
            }
        }

        // Op 3: Duplicate Detection (content hash)
        let mut seen_hashes: HashMap<u64, String> = HashMap::new();
        let all_memories = self.memory.get_all(agent_id);
        for entry in &all_memories {
            let hash = hash_content(&entry.content);
            if let Some(existing_id) = seen_hashes.get(&hash) {
                if entry.created_at_ms < self.memory.get(agent_id, existing_id)
                    .map(|e| e.created_at_ms).unwrap_or(0)
                {
                    self.memory.remove(agent_id, &entry.id);
                    deduplicated += 1;
                }
            } else {
                seen_hashes.insert(hash, entry.id.clone());
            }
        }

        ConsolidationReport {
            expired,
            promoted,
            deduplicated,
            orphans_flagged: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }
}
```

**CLI 反馈**:

```bash
> aicli session-end --agent dev --session abc123
Session 'abc123' ended.
Memory consolidation: 3 expired, 1 promoted, 0 deduplicated (2ms).
```

**Soul 对齐**:
- 公理 3（记忆跨越边界）— 整合确保记忆质量随时间提升而非退化
- 公理 5（机制不是策略）— OS 执行 TTL/频率规则，不判断记忆价值
- 公理 9（越用越好）— 每次 session-end 都让记忆系统更干净

**验收标准**:
```bash
aicli session-start --agent dev           # 获得 session_id
aicli remember --agent dev --content "X"  # 存储
aicli remember --agent dev --content "X"  # 重复
aicli session-end --agent dev --session <sid>
# 输出包含: "1 deduplicated"
aicli recall --agent dev                  # 不再包含重复记忆
```

---

### A-6: Context Honest Degradation — 上下文诚实降级

**痛点**: B13 — `context.load L0` 返回全文但声称自己是 "L0"。Agent 无法区分真摘要和假摘要。

**根因分析**:

```
context_loader.rs:148 — compute_l0 对 ≤20 word 的内容直接返回原文
context_loader.rs:160 — load_l0 始终返回 layer: ContextLayer::L0

问题不在于逻辑（短内容不需要摘要是合理的），
而在于返回值不反映真实的降级状态。
```

**设计**: 在 `LoadedContext` 中添加降级指示器。

```rust
// src/fs/context_loader.rs — 扩展 LoadedContext
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadedContext {
    pub cid: String,
    pub layer: ContextLayer,
    pub content: String,
    pub tokens_estimate: usize,
    pub actual_layer: ContextLayer,          // 实际返回的数据层级
    pub degraded: bool,                      // 是否降级
    pub degradation_reason: Option<String>,  // 降级原因
}
```

**降级判定逻辑**:

```rust
impl ContextLoader {
    fn load_l0(&self, cid: &str) -> std::io::Result<LoadedContext> {
        // ... existing cache/disk load logic ...

        let (content, actual_layer, degraded, reason) = match fs::read_to_string(&path) {
            Ok(s) => (s, ContextLayer::L0, false, None),
            Err(_) => {
                let raw = self.cas.get(cid)
                    .map(|obj| String::from_utf8_lossy(&obj.data).into_owned())
                    .unwrap_or_default();
                let words: Vec<&str> = raw.split_whitespace().collect();

                if words.len() <= 20 {
                    (raw, ContextLayer::L2, true,
                     Some("Content too short for summarization; returning full text".into()))
                } else if self.summarizer.is_some() {
                    match self.summarizer.as_ref().unwrap().summarize(&raw, SummaryLayer::L0) {
                        Ok(summary) => (summary, ContextLayer::L0, false, None),
                        Err(e) => {
                            let heuristic = self.compute_l0_heuristic(&raw);
                            (heuristic, ContextLayer::L0, true,
                             Some(format!("LLM summarizer failed: {}; using heuristic", e)))
                        }
                    }
                } else {
                    let heuristic = self.compute_l0_heuristic(&raw);
                    (heuristic, ContextLayer::L0, true,
                     Some("No LLM summarizer available; using heuristic (first+last 10 words)".into()))
                }
            }
        };

        Ok(LoadedContext {
            cid: cid.to_string(),
            layer: ContextLayer::L0,
            content,
            tokens_estimate: /* ... */,
            actual_layer,
            degraded,
            degradation_reason: reason,
        })
    }
}
```

**CLI 反馈**:

```bash
> aicli context --cid QmABC --layer L0
Layer: L0 (degraded → L2: Content too short for summarization)
Content: "The quick brown fox jumps over the lazy dog"
Tokens: ~9
```

**Soul 对齐**:
- 公理 1（Token 最稀缺）— Agent 知道 L0 质量后，可以决定是否加载 L1 补充
- 公理 6（结构先于语言）— 降级信息是结构化字段，非自然语言解释

**验收标准**:
```bash
aicli put --content "short" --tags "test"   # 短内容
aicli context --cid <cid> --layer L0
# 输出包含: degraded=true, reason="Content too short..."

aicli put --content "<2000字长文>" --tags "test"  # 长内容
aicli context --cid <cid> --layer L0
# 无 LLM 时: degraded=true, reason="No LLM summarizer..."
# 有 LLM 时: degraded=false
```

---

### A-7: System Self-Report — 系统自检报告

**来源**: L-9（Node 11 未实现）+ F-41（Node 9 未实现）

**设计**: 新增 MCP resource `plico://health` 和 CLI 命令 `aicli health`，返回结构化的系统完整度报告。

**数据结构**:

```rust
// src/api/semantic.rs — 新增
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealthReport {
    pub timestamp_ms: u64,
    pub overall_status: HealthStatus,
    pub subsystems: Vec<SubsystemHealth>,
    pub capabilities: Vec<CapabilityStatus>,
    pub known_degradations: Vec<Degradation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HealthStatus { Healthy, Degraded, Critical }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemHealth {
    pub name: String,           // "cas", "memory", "kg", "search", "events"
    pub status: HealthStatus,
    pub object_count: u64,
    pub last_write_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityStatus {
    pub name: String,           // "vector_search", "bm25_search", "embedding", "summarization"
    pub available: bool,
    pub mode: String,           // "full", "stub", "degraded", "unavailable"
    pub detail: Option<String>, // "Using stub embedding; BM25 only"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Degradation {
    pub component: String,
    pub severity: String,       // "warning", "error"
    pub description: String,
    pub since_ms: Option<u64>,
    pub suggestion: Option<String>,
}
```

**实现路径**:

```rust
// src/kernel/ops/dashboard.rs — 新增
impl AIKernel {
    pub fn health_report(&self) -> SystemHealthReport {
        let now = now_millis();
        let indicators = self.health_indicators();

        let embedding_mode = match self.fs.embedding_backend_name() {
            "stub" => ("stub", false, "BM25 only; no vector search"),
            "local" => ("full", true, "Local ONNX embedding"),
            "ollama" => ("full", true, "Ollama embedding"),
            other => ("unknown", false, other),
        };

        let subsystems = vec![
            SubsystemHealth {
                name: "cas".into(),
                status: HealthStatus::Healthy,
                object_count: self.fs.cas_object_count() as u64,
                last_write_ms: None,
            },
            SubsystemHealth {
                name: "knowledge_graph".into(),
                status: if self.fs.knowledge_graph.is_some() {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Degraded
                },
                object_count: self.fs.kg_node_count() as u64,
                last_write_ms: None,
            },
            // ... memory, events, search subsystems
        ];

        let mut degradations = Vec::new();
        if embedding_mode.0 == "stub" {
            degradations.push(Degradation {
                component: "embedding".into(),
                severity: "warning".into(),
                description: "Vector search unavailable; using BM25 fallback only".into(),
                since_ms: None,
                suggestion: Some("Set EMBEDDING_BACKEND=local or ollama".into()),
            });
        }

        if indicators.event_log_pressure > 0.8 {
            degradations.push(Degradation {
                component: "event_log".into(),
                severity: "warning".into(),
                description: format!("Event log pressure: {:.0}%", indicators.event_log_pressure * 100.0),
                since_ms: None,
                suggestion: Some("Events will auto-rotate; no action needed".into()),
            });
        }

        SystemHealthReport {
            timestamp_ms: now,
            overall_status: if degradations.iter().any(|d| d.severity == "error") {
                HealthStatus::Critical
            } else if !degradations.is_empty() {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            },
            subsystems,
            capabilities: vec![
                CapabilityStatus {
                    name: "vector_search".into(),
                    available: embedding_mode.1,
                    mode: embedding_mode.0.into(),
                    detail: Some(embedding_mode.2.into()),
                },
                // ... bm25, summarization, kg capabilities
            ],
            known_degradations: degradations,
        }
    }
}
```

**MCP Resource**:

```
plico://health  →  SystemHealthReport JSON
```

**CLI 命令**:

```bash
> aicli health
System Status: Degraded

Subsystems:
  cas:              Healthy (140 objects)
  knowledge_graph:  Healthy (141 nodes)
  memory:           Healthy (4 tiers active)
  events:           Healthy (pressure: 12%)

Capabilities:
  vector_search:    unavailable (stub mode)
  bm25_search:      available
  embedding:        stub (BM25 only)
  summarization:    unavailable (no LLM configured)

Degradations:
  ⚠ embedding: Vector search unavailable; using BM25 fallback only
    → Set EMBEDDING_BACKEND=local or ollama
```

**Soul 对齐**:
- 公理 7（主动先于被动）— Agent 启动时即可获取系统能力清单
- 公理 1（Token 最稀缺）— Agent 知道 embedding 是 stub 模式后，可以跳过语义搜索

**验收标准**:
```bash
EMBEDDING_BACKEND=stub aicli health     # → 显示 "Degraded" + embedding warning
EMBEDDING_BACKEND=local aicli health    # → 显示 "Healthy"（如果 ONNX 可用）
# MCP: plico://health                  # → 结构化 JSON
```

---

### A-8: Contract Completion Sprint — 契约清债冲刺

**目标**: 批量修复 9 个 Bug（含 3 个本轮新发现），将 Agentic CLI 通过率从 66% 提升至 85%+。

每个修复都是小范围代码变更，合计 ~150 行：

#### A-8a: tag-only search 修复 (B25 — NEW, HIGH)

**Dogfood 复现**: `search --tags architecture` → "No results"（4 个对象有此 tag）

```rust
// src/bin/aicli/commands/handlers/crud.rs — cmd_search
// 当只有 tags 无 query 时, 应走 tag index 而非 BM25
pub fn cmd_search(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query");
    let tags = extract_tags(args, "--tags");
    match (query, tags.is_empty()) {
        (Some(q), _) => /* existing BM25 search path */,
        (None, false) => {
            // Tag-only search: use tag index directly
            let results = kernel.search_by_tags(&tags);
            /* format results */
        }
        (None, true) => ApiResponse::error("--query or --tags required"),
    }
}
```

#### A-8b: events history --agent 参数名统一 (B14)

**Dogfood 复现**: `events history --agent <UUID>` → 返回全部 9 events；`--agent-filter` → 正确返回 4 events

```rust
// src/bin/aicli/commands/handlers/events.rs — events history
// 统一: 同时读取 --agent 和 --agent-filter, 兼容两种写法
let agent_id_filter = extract_arg(args, "--agent-filter")
    .or_else(|| extract_arg(args, "--agent"));
```

#### A-8c: tool describe 参数 schema (B16)

```rust
// src/bin/aicli/commands/handlers/tool.rs
// 修改 cmd_tool_describe: 从 ToolDescriptor 提取 input_schema 并输出
pub fn cmd_tool_describe(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let tool_name = args.get(2).cloned().unwrap_or_default();
    match kernel.tool_describe(&tool_name) {
        Some(desc) => {
            let mut r = ApiResponse::ok();
            r.tool_schema = Some(desc.input_schema.clone());
            r.data = Some(format!("{}: {}", desc.name, desc.description));
            r
        }
        None => ApiResponse::error_with_diagnosis(
            format!("Tool not found: {}", tool_name),
            "TOOL_NOT_FOUND",
            "Check available tools",
            vec!["plico(tool=\"list\")".into()],
        ),
    }
}
```

#### A-8d: tool call 不存在工具的错误处理 (B19)

```rust
// src/kernel/builtin_tools.rs — execute_tool 中不存在工具的分支
// 返回明确错误而非空结果
ToolResult::error(format!(
    "Tool '{}' not found. Available tools: {}",
    tool_name,
    self.tools.keys().take(5).cloned().collect::<Vec<_>>().join(", ")
))
```

#### A-8e: hybrid preview 内容而非 CID (B23)

**注**: Dogfood 实测中 hybrid preview 显示的是真实内容（3 个都是文本），但审计报告称部分显示 CID。可能与数据有关——保留修复但降低优先级。

```rust
// src/kernel/ops/hybrid.rs — HybridHit preview 填充
// 当 preview 为空或等于 CID 时，从 CAS 加载实际内容前 100 字符
if hit.preview.is_empty() || hit.preview == hit.cid {
    if let Some(obj) = self.fs.get_raw(&hit.cid) {
        let text = String::from_utf8_lossy(&obj.data);
        hit.preview = text.chars().take(100).collect();
    }
}
```

#### A-8f: message.send 修复 (B24)

**Dogfood 复现**: `send --to <UUID> --body "hello"` → exit 1 无输出。比审计报告更严重——不仅无输出，操作直接失败。

```rust
// src/bin/aicli/commands/handlers/messaging.rs
// cmd_send 成功后返回确认消息
let mut r = ApiResponse::ok();
r.message = Some(format!("Message sent to '{}' (id: {})", to, msg_id));
r
```

#### A-8g: tool call limit/去重修复 (B26 — NEW)

**Dogfood 复现**: `tool call cas.search '{"query":"circuit","limit":3}'` → 返回 10 条 + 重复 CID

```rust
// src/kernel/builtin_tools.rs — cas.search tool handler
// 尊重 limit 参数 + 对结果去重
let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
let mut seen = HashSet::new();
results.retain(|r| seen.insert(r.cid.clone()));
results.truncate(limit);
```

#### A-8h: KG explore edge type 修正 (B17)

```rust
// src/fs/graph/types.rs — KGEdgeType Display 实现
// 确保 RelatedTo 序列化为 "related_to" 而非 "relatedto"
impl fmt::Display for KGEdgeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KGEdgeType::RelatedTo => write!(f, "related_to"),
            // ... other variants
        }
    }
}
```

**验收标准（整体）**:

```bash
# A-8a: tag-only search 返回结果
aicli search --tags architecture          # → ≥1 results (非 "No results")

# A-8b: events history --agent 有效
aicli events history --agent <UUID>       # → 只返回该 agent 的事件

# A-8c: tool describe 显示 schema
aicli tool describe cas.search            # → 显示参数 schema

# A-8d: tool call 不存在工具
aicli tool call nonexistent               # → Error + exit 1

# A-8f: message.send 工作
aicli send --to <UUID> --body "hello"     # → exit 0 + 确认消息

# A-8g: tool call 尊重 limit
aicli tool call cas.search '{"query":"x","limit":2}'  # → ≤2 results, 无重复

# A-8h: explore 边类型正确
aicli explore --node <id>                 # → "related_to" 非 "relatedto"
```

---

## 6. Node 9/10 剩余特性处置

Node 12 同时解决了 Node 9 和 Node 10 的部分遗留特性：

| 原始特性 | Node 12 覆盖 | 处置 |
|----------|-------------|------|
| F-36: BM25 Scoring 优化 | — | **延后**: 当前 BM25 功能正常，优化是 nice-to-have |
| F-37: Search Snippet | DTO 已有字段 | **已实现**（DTO 层面）|
| F-41: Degradation Visibility | **A-6 覆盖** | ✅ SystemHealthReport + Degradation |
| F-42: CAS Access Lazy Persist | — | **延后**: 当前 Access Tracking 工作正常 |
| F-46: Context 诚实降级 | **A-5 覆盖** | ✅ LoadedContext 添加 degradation 字段 |
| F-48: 结构化错误诊断 | **A-7b 部分覆盖** | ⚠️ error_with_diagnosis 已有框架 |
| F-50: tool describe schema | **A-7a 覆盖** | ✅ |

**覆盖率**: Node 9 剩余 4 个 → 解决 1 个（F-41），延后 3 个。Node 10 剩余 → 解决 3 个（F-46, F-48 部分, F-50）。

---

## 7. 实施计划

### Sprint 1: 会话 + 身份修复 (4 天) — CRITICAL

| 任务 | 预估 | 文件 | 修复 Bug |
|------|------|------|----------|
| A-1: Session Persistence | ~100 行 | kernel/ops/session.rs, kernel/persistence.rs | B22, B20 |
| A-2: Agent Name Registry | ~80 行 | scheduler/mod.rs, kernel/ops/agent.rs | B21 |
| A-8a: tag-only search | ~30 行 | bin/aicli/commands/handlers/crud.rs | B25 |
| A-8b: events --agent 统一 | ~3 行 | bin/aicli/commands/handlers/events.rs | B14 |
| 测试 | ~80 行 | tests/session_persist_test.rs, tests/agent_name_test.rs | |

**Sprint 1 交付**: session-end 能找到 session。growth Sessions > 0。Agent 可按名字查询。tag search 正常。

**Sprint 1 Dogfood 验收**:
```bash
aicli session-start --agent <UUID>         # → Session started: <SID>
aicli session-end --agent <UUID> --session <SID>  # → Session ended (非 "not found")
aicli growth --agent <UUID>                # → Sessions: >= 1
aicli status --agent test-agent-alpha      # → Agent state: ... (非 exit 1)
aicli search --tags architecture           # → ≥1 results
aicli events history --agent <UUID>        # → 只返回该 agent 事件
```

### Sprint 2: 契约清债 + 系统自省 (3 天)

| 任务 | 预估 | 文件 | 修复 Bug |
|------|------|------|----------|
| A-6: Context Honest Degradation | ~40 行 | fs/context_loader.rs | B13 |
| A-7: System Self-Report | ~120 行 | kernel/ops/dashboard.rs, api/semantic.rs | L-9, F-41 |
| A-8c: tool describe schema | ~20 行 | bin/aicli/commands/handlers/tool.rs | B16 |
| A-8d: tool call error | ~10 行 | kernel/builtin_tools.rs | B19 |
| A-8f: message.send | ~15 行 | bin/aicli/commands/handlers/messaging.rs | B24 |
| A-8g: tool call limit/dedup | ~15 行 | kernel/builtin_tools.rs | B26 |
| A-8h: edge type naming | ~5 行 | fs/graph/types.rs | B17 |
| 测试 | ~60 行 | 分散 | |

**Sprint 2 交付**: plico://health resource。Context L0 降级标记。剩余契约修复。

### Sprint 3: 记忆进化 + Agent Card (4 天)

| 任务 | 预估 | 文件 | 依赖 |
|------|------|------|------|
| A-3: Agent Card | ~80 行 | scheduler/agent.rs, api/semantic.rs | A-2 |
| A-4: Memory Link Engine | ~80 行 | kernel/ops/memory.rs, event_bus.rs | — |
| A-5: Memory Consolidation Cycle | ~120 行 | kernel/ops/memory.rs, session.rs | A-1 |
| 测试 | ~80 行 | tests/memory_link_test.rs, tests/consolidation_test.rs | |

**Sprint 3 交付**: 记忆自动链接。session-end 触发整合。Agent Card。

### 总代码量预估

```
新增代码:   ~620 行 Rust
新增测试:   ~220 行
修改代码:   ~160 行（现有文件修改）
总计:       ~1000 行

新增外部依赖: 0
新增二进制:   0
新增文件:     3-4 (测试文件)
```

---

## 8. 验收评分卡

### Dogfood 回归测试（35 项 + 8 项新增 = 43 项）

```
原有 35 项 (当前: 23 通过, 10 失败, 2 部分):
  修复目标:
    B22 session-end → ✅ (A-1)     +1
    B20 growth → ✅ (A-1)           +1
    B21 status by name → ✅ (A-2)   +1
    B14 events --agent → ✅ (A-8b)  +1
    B13 context L0 → ✅ (A-6)       +1
    B16 tool describe → ✅ (A-8c)   +1
    B19 tool call error → ✅ (A-8d) +1
    B24 message.send → ✅ (A-8f)    +1
  目标: 31/35 通过 (89%)

新增 8 项:
  36. Session lifecycle round-trip    (A-1)
  37. Agent name resolution           (A-2)
  38. Agent Card set/get              (A-3)
  39. Memory linking feedback         (A-4)
  40. Session-end consolidation       (A-5)
  41. Context degradation indicator   (A-6)
  42. plico://health resource         (A-7)
  43. tag-only search                 (A-8a)

目标通过率: 37/43 (86%) — 从 66% 提升至 86%
```

### Soul 2.0 覆盖矩阵

| 公理 | A-1 | A-2 | A-3 | A-4 | A-5 | A-6 | A-7 | A-8 |
|------|-----|-----|-----|-----|-----|-----|-----|-----|
| 1. Token 稀缺 | | | | | ✅ | ✅ | ✅ | |
| 2. 意图先于操作 | | ✅ | | | | | | |
| 3. 记忆跨越边界 | ✅ | | | ✅ | ✅ | | | |
| 4. 共享先于重复 | | | ✅ | ✅ | | | | |
| 5. 机制不是策略 | ✅ | ✅ | | ✅ | ✅ | | | |
| 6. 结构先于语言 | | | ✅ | | | ✅ | ✅ | |
| 7. 主动先于被动 | | | | ✅ | ✅ | | ✅ | |
| 8. 因果先于关联 | | | | ✅ | | | | |
| 9. 越用越好 | ✅ | | ✅ | ✅ | ✅ | | | |
| 10. 会话一等公民 | **✅** | | | | ✅ | | | |

**公理 10 高亮**: A-1 Session Persistence 是 Node 12 最重要的修复——让公理 10 在 CLI 模式下真正兑现。

---

## 9. AIOS 路线图定位

```
AIOS 能力谱系                          Plico 覆盖
─────────────────────                  ──────────────
□ Agent Scheduling                     ✅ AgentScheduler + DispatchLoop
□ Memory Management (RAM)              ✅ 4-tier LayeredMemory
□ Storage Management (Disk)            ✅ CAS + EventLog + KG persistence
□ Context Management                   ✅ L0/L1/L2 + ContextBudget
□ Tool Management                      ✅ ToolRegistry + MCP + Safety Rails
□ Access Control                       ✅ PermissionGuard + tenant isolation
□ Agent Identity Resolution            ❌ → A-1 填补
□ Agent Discovery (Card)               ❌ → A-2 填补
□ Memory Evolution (A-MEM)             ❌ → A-3 填补
□ Memory Consolidation (All-Mem)       ❌ → A-4 填补
□ System Introspection                 ❌ → A-6 填补
□ Context Degradation Visibility       ❌ → A-5 填补
□ Memory Compression (LLM-based)       ❌ → Node 13 候选
□ Persistent Daemon (cold start fix)   ⚠️ → Node 13 候选
□ Agent-to-Agent Protocol (A2A)        ❌ → Node 14+ 候选
```

**Node 12 将填补 7 项 AIOS 能力空白 + 修复 1 项架构缺陷**，使 Plico 在会话生命周期、Agent 身份、记忆进化、系统自省四个维度对齐 2026 前沿。

---

## 10. Node 13 前瞻（觉知之后）

Node 12 建立了觉知的基础机制层。Node 13 的自然方向是 **深度整合**：

```
Node 13 候选方向（按 AIOS 前沿排序）:

1. Memory Compression (F-26 最终实现)
   A-4 建立了整合框架 → Node 13 添加 LLM 驱动的记忆摘要/合并
   对标: All-Mem 的 LLM diagnoser + confidence-gated topology edits

2. Persistent Daemon Mode
   plicod 已存在但 CLI 是 process-per-command
   → CLI→Daemon 通信改为 TCP 客户端模式
   → 消除冷启动延迟（4-5s → <100ms）

3. Agent-to-Agent Communication (A2A)
   A-1 建立了名字解析 + A-2 建立了 Agent Card
   → 自然延伸到 A2A 式跨 Agent 任务委派
   → 对标: MCP Tasks primitive (SEP-1686)

4. Proactive Context Assembly
   A-6 建立了系统状态感知
   → OS 在 session-start 时基于 Agent profile 预组装最优上下文
   → 公理 7 的深度实现
```

---

*设计基于 808 个测试通过的代码基线 + AI Agent 真实 CLI Dogfood 实测。*
*v2.0 校正: 不依赖 git log 推断修复状态，全部结论来自命令 exit code + stdout/stderr。*
*关键校正: B22 根因是 session 不跨进程持久化（架构级），非 name 解析问题。*
*新发现: B25 tag-only search 断裂, B26 tool call limit/去重缺失, B27 edge 节点名空。*
*八个特性: 1 架构修复 + 1 身份修复 + 1 Agent Card + 2 记忆进化 + 1 上下文/自省 + 1 系统报告 + 1 契约清债。*
*零新外部依赖。零新二进制。~1000 行代码。11 天交付。*
