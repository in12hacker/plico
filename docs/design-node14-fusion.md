# Plico 第十四节点设计文档
# 融 — 子系统融合与记忆绑定

**版本**: v2.0（功能全量审计 + Harness/Harmless 校正版）
**日期**: 2026-04-22
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 记忆完整 + 接口一致 + 记忆绑定 + 降级通路 + **自验 (Harness)**
**前置**: 节点 13 ✅（60%） / 节点 12 ✅（50%）
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n14-dogfood`）+ **功能全量代码审计**（非仅 bug 视角）+ AIOS 2026 前沿对标 + **Harness/Harmless 信息校正层**
**信息来源**: `docs/dogfood-audit-node1-13.md` + Kumiho / TierMem / MemMachine / OrKa Memory Binding / HugRAG / ProGraph-R1 + **ABC (Agent Behavioral Contracts) / CAAF (Harness as Asset) / AgentSys / Springdrift / VIGIL / MARIA VITAL / SuperLocalMemory**

> **v2.0 vs v1.0 变更说明**
>
> v1.0 仅从 bug 报告出发推演特性，是**损伤驱动（damage-driven）**设计。
> v2.0 新增：
> 1. **功能全量审计** — 不仅看"什么坏了"，更看"什么存在但未连通"
> 2. **AI 第一人称重新推演** — 论文校准从人类视角转译为 AI 认知视角
> 3. **Harness/Harmless 校正层** — 引入自验证维度，对标 ABC/CAAF/AgentSys/Springdrift

---

## 0. Dogfood 实测校正：审计报告的独立验证

> **以下全部结论来自 AI Agent 在 `/tmp/plico-n14-dogfood` 全新实例上的真实 CLI 执行。**
> **不使用管道（消除 `$?` 捕获误差）。每条命令独立运行后立即检查 exit code。**

### 审计报告 vs 独立 Dogfood 对比

```
Bug    报告状态            独立实测                         代码行级根因
────── ─────────────────── ──────────────────────────────── ────────────────────────────
B35    🔴 CRITICAL panic   未复现 — delete 无 panic          权限正确拦截, exit 1
                                                            可能已修复或环境特异

B38    HIGH tier 忽略      ✅ 确认                          memory.rs:43 cmd_recall
                           recall --tier working/long-term   不提取 --tier, 直接调用
                           /ephemeral/procedural 输出完全     kernel.recall() 无过滤
                           相同 (只显示 Working+LongTerm)

B39    HIGH paths 空       未确认 — paths 有 edge 时正常     可能报告测试时 edge 未创建
                           返回 3 条路径

B40    MEDIUM tool recall  ✅ 确认                          builtin_tools.rs:358
                           {"memories":[]} 空数组             execute_tool(agent_id)
                           CLI recall 同 agent 有结果        agent_id 来自调用者上下文
                                                            非 JSON 参数

B41    MEDIUM require-tags ✅ 确认 + 根因定位                crud.rs:43-49
                           无 --query 时返回 "No results"    --require-tags flag 被解析
                           有 --query 时正确过滤              为 positional query 文本
                                                            BM25 搜索 "--require-tags" → 0 结果

B42    MEDIUM MCP recall   ✅ 确认                          plico_mcp.rs recall_semantic
                           "Server unavailable"              直接调用 embedding, 无 BM25 fallback

A-6    ❌ context L0       ✅ 实际已修复!                    context_loader.rs
                           [L0, degraded from L2]            正确标记降级 + 原因
                           + Degradation 原因输出

额外    ephemeral/proc     ✅ 确认                          kernel.recall() 只返回
                           4 条记忆存入, 只回调 2 条          Working + LongTerm
                           (ephemeral + procedural 丢失)     不含 Ephemeral/Procedural
```

### 校正后的系统评分

```
修正项:
  B35: 未复现 → 移除 CRITICAL 标记 (降为 LOW/monitor)
  B39: paths 实测正常 → 移除 (可能报告条件不同)
  A-6: context L0 已修复 → Node 12 通过率 +1

校正后按能力维度:
  分层记忆:    43% → 未改善 (B38 仍存在, ephemeral/procedural 仍不可回调)
  上下文装配:  33% → 67% (A-6 已修复)
  CAS/搜索:   88% → 未改善 (B41 仍存在)
  工具系统:    80% → 未改善 (B40 仍存在)
  MCP 协议:   75% → 未改善 (B42 仍存在)

最弱环节: 分层记忆 (43%) — 这是 Plico 作为 AIOS 的核心承诺
```

---

## 0.5 功能全量审计：超越 Bug 的架构发现

> **v1.0 只看 bug（什么坏了），v2.0 同时看功能（什么存在但未连通）。**
> 以下是逐文件功能审计发现的 9 个架构级问题——它们不是 bug，而是**承诺与实现的结构性断裂**。

### F-A: Ephemeral 记忆持久化空洞 ⚠️ DESIGN

```
代码路径:
  remember --tier ephemeral
    → cmd_remember → _ => kernel.remember()
    → memory.rs:57 kernel.remember()
    → memory.store_checked(entry) ← 存入内存 ✅
    → 缺少 self.persist_memories() ← 未写入磁盘 ❌

对比:
  kernel.remember_working()  → self.persist_memories() ← 写入磁盘 ✅
  kernel.remember_long_term() → self.persist_memories() ← 写入磁盘 ✅

后果: aicli 每个命令是独立进程。
  remember --tier ephemeral → 存入内存 → 进程退出 → 数据丢失
  recall → 新进程启动 → 从磁盘加载 → 无 ephemeral 数据
```

**根因不是 bug，是架构决策**：Ephemeral = volatile cache = 不应持久化。但 CLI 模式每个命令是独立进程，这个决策让 Ephemeral 层在 CLI 下 **完全不可用**。

### F-B: Procedural CLI 路由错接 ⚠️ DESIGN

```
代码路径:
  remember --tier procedural
    → cmd_remember → parse_memory_tier("procedural") = MemoryTier::Procedural
    → match arm: _ => kernel.remember(agent_id, scope, content)
    → 实际存为 MemoryTier::Ephemeral (importance=50) ← 错!

预期: 应调用 kernel.remember_procedural() → 存为 MemoryTier::Procedural
实际: 走 kernel.remember() 通用路径 → 存为 Ephemeral → 且不持久化 (F-A)

双重失效: 存入错误 tier + 不写磁盘 = Procedural 完全不可达
```

### F-C: Consolidation 报告存在但未显示 ⚠️ WIRING

```
内核层:
  end_session_orchestrate()
    → TierMaintenance::run_maintenance_cycle() ← 执行维护 ✅
    → MaintenanceStats { promoted, evicted, linked } ← 收集统计 ✅
    → 返回 ConsolidationReport ← 结构化报告 ✅

API 层:
  SessionEnded.consolidation: Option<ConsolidationReport> ← 字段存在 ✅

CLI 输出层:
  mod.rs:393-399 → 打印 "Session ended" + last_seq ← 只输出 2 个字段
  → consolidation 字段完全被忽略 ← ❌

结论: 整合逻辑已实现，统计已收集，报告已生成——唯独 CLI 不显示。
      审计报告说 "无可观察整合效果" 是正确的——因为输出被吞了。
```

### F-D: Memory-KG Linking 范围窄 + 无内容匹配 ⚠️ SCOPE

```
link_memory_to_kg() 功能审计:
  ✅ 创建 Memory 类型 KG 节点
  ✅ 遍历同 agent 已有 Memory 节点
  ✅ 基于 tag 交集创建 SimilarTo 边
  ❌ 只在 LongTerm 分支调用 (memory.rs:28)
  ❌ 仅用 tag 匹配，不用 BM25 内容相似度
  ❌ 无 tag 的记忆 → 创建节点但永远没有边
```

### F-E: TierMaintenance 基础设施完备但与 Session 断路 ⚠️ WIRING

```
已存在:
  TierMaintenance::run_maintenance_cycle() → MaintenanceStats ← 完整
  PromotionThresholds: Ephemeral→Working(access>=3), Working→LongTerm(access>=10,imp>=50) ← 完整
  evict_ephemeral: importance<70 丢弃，>=70 提升到 Working ← 完整
  MaintenanceStats.linked_count ← 字段存在，但永远为 0 ("set by caller")

断路:
  session-end 调用 maintenance → stats 返回 → 但 linked_count 永远是 0
  因为 end_session_orchestrate 不做 memory→KG linking
```

### F-F: Auto-Checkpoint 未实现 ⚠️ STUB

```
end_session_orchestrate() line 520-528:
  auto_checkpoint 参数接收了 → 但逻辑是:
  let checkpoint_id = if auto_checkpoint { None } else { None };
  // 注释: "For now, we rely on the client's explicit AgentCheckpoint call"
```

### F-G: Session-End 清除全部 Ephemeral ⚠️ BY DESIGN

```
end_session_orchestrate() line 544:
  let _cleared = memory.clear_ephemeral(agent_id);
  // 维护周期后，剩余的 ephemeral 全部清除

结合 F-A: ephemeral 既不持久化(CLI模式丢失)，
         session-end 又全部清除(daemon模式也丢失)。
         Ephemeral 层是一个结构性死亡区。
```

### F-H: MemoryQuery 类型有 tier 字段但从未使用 ⚠️ DEAD CODE

```
memory/mod.rs:
  pub struct MemoryQuery {
      pub tier: Option<MemoryTier>,  // ← 存在
      ...
  }

kernel/ops/memory.rs → recall() → memory.get_all() → 不使用 MemoryQuery → 无 tier 过滤
```

### F-I: recall_semantic vs recall_relevant_semantic 降级差异 ⚠️ INCONSISTENCY

```
kernel.recall_semantic():
  embedding.embed(query) → Err → 返回 Err ← 无 fallback ❌

kernel.recall_relevant_semantic():
  embedding.embed(query) → Err → fallback 到 recall_relevant() ← 有 fallback ✅

MCP 用的是 recall_semantic (无 fallback) → B42 的真正根因
```

### 功能审计总结：断裂模式

```
模式 1: "存在但未连通"
  TierMaintenance 存在但 linked_count=0
  ConsolidationReport 生成但 CLI 不显示
  MemoryQuery.tier 存在但 recall 不使用

模式 2: "路由错接"
  --tier procedural → 实际存为 Ephemeral
  tool call agent_id → 实际用 caller context
  --require-tags → 实际变成 query 文本

模式 3: "模式不兼容"
  Ephemeral = volatile cache (daemon 模式设计)
  CLI = stateless (每命令独立进程)
  两者组合 = Ephemeral 在 CLI 下完全失效
```

---

## 1. 为什么叫"融"：从 Memory Binding 到 AIOS

### 生物学演进

```
Node 7  代谢    — 能量管理
Node 8  驾具    — 工具使用
Node 9  韧性    — 免疫系统
Node 10 正名    — 名实一致
Node 11 落地    — 安装设计
Node 12 觉知    — 意识觉醒
Node 13 通      — 信号传导
Node 14 融      — 功能整合
```

**融（Fusion）是什么？**

在神经科学中，"功能整合"是大脑从独立模块协作到统一认知的关键阶段。视觉皮层看到颜色，听觉皮层听到声音，但只有当二者**绑定**在一起时，"红色警报声"才有意义。这就是**绑定问题（Binding Problem）**。

Plico 此刻面临完全相同的绑定问题：

```
┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
│  4 层记忆     │  │  知识图谱     │  │  CLI 接口     │  │  Tool API    │
│  Ephemeral   │  │  KG nodes    │  │  cmd_recall   │  │  memory.recall│
│  Working     │  │  KG edges    │  │  cmd_search   │  │  cas.search  │
│  LongTerm    │  │  KG paths    │  │  cmd_context  │  │  recall_sem  │
│  Procedural  │  │              │  │              │  │              │
├──────────────┤  ├──────────────┤  ├──────────────┤  ├──────────────┤
│ 🔴 tier 过滤  │  │ 🔴 与记忆     │  │ 🔴 与 tool API│  │ 🔴 agent_id  │
│   不工作     │  │   不链接      │  │   结果不一致   │  │   参数被忽略  │
│ 🔴 eph/proc  │  │              │  │              │  │ 🔴 MCP 无     │
│   不可回调   │  │              │  │              │  │   fallback   │
└──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘
         ↑                ↑                ↑                ↑
         └────────────── 四个独立子系统，互不绑定 ─────────────┘
```

OrKa Brain 的开发者在 500 次记忆实验后发现了同样的问题：

> "Two complete memory systems, sitting in the same codebase, sharing no information.
> I had built the hippocampus and the motor cortex as two separate systems that had never met."

这就是 Node 14 要解决的：**让子系统融合，让记忆绑定**。

### 与前序节点的区别

| 维度 | Node 12（觉知） | Node 13（通） | Node 14（融） |
|------|-----------------|---------------|---------------|
| 关注点 | 意识到自身 | 信号忠实传导 | 子系统协调融合 |
| 典型问题 | 不知道自己是谁 | 参数不到达 | 子系统互不通信 |
| 修复方式 | 身份机制 | 通路修复 | 绑定链接 |
| 生物类比 | 意识觉醒 | 神经传导 | **功能整合** |

---

## 2. AIOS 2026 前沿校准

### 2.1 记忆架构前沿

| 系统 | 核心创新 | Plico 现状 | Node 14 行动 |
|------|----------|-----------|-------------|
| **Kumiho** (arXiv:2603.17244, Mar 2026) | Graph-native cognitive memory + formal belief revision (AGM postulates). Dual-store: Redis working + Neo4j long-term. Prospective indexing. | KG + 4-tier 存在但互不链接; B38 tier 过滤断裂 | **F-1/F-2**: 修复 tier recall; **F-5**: Memory-KG 绑定 |
| **TierMem** (arXiv:2602.17913, Feb 2026) | Provenance-linked two-tier. Runtime sufficiency router. Escalation: summary → raw log. | 4 tier 无 provenance; tier 间无链接; 无 escalation | **F-1**: tier recall 是 escalation 的前提 |
| **MemMachine** (arXiv:2604.04853, Apr 2026) | STM + episodic + profile. Ground-truth-preserving: raw episodes + contextualized retrieval. | CAS = ground truth; 但 recall 不返回全部 tier | **F-2**: 全 tier 可回调 |
| **Memory Binding** (OrKa Brain, 2026) | 核心洞见: Skill + Episode 两套系统不绑定 = 记忆失效. 需要 Memory Bundle. | 记忆与 KG 两套系统不绑定 | **F-5**: remember → KG edge |
| **Production Memory** (Chaitanya, 2026) | 5 层: working/episodic/semantic/procedural + 各 tier 不同存取策略. Memory as MCP service. | 4 tier 但 recall 不按 tier 过滤 | **F-1/F-2/F-8** |

### 2.2 GraphRAG 前沿

| 系统 | 核心创新 | Plico 现状 | Node 14 行动 |
|------|----------|-----------|-------------|
| **HugRAG** (arXiv:2602.05143) | 层级因果知识图谱 — causal gating 区分因果 vs 虚假关联 | KG 有 CausedBy/DependsOn, 但 memory 不触发 edge | **F-5**: remember → KG causal edge |
| **ProGraph-R1** (arXiv:2601.17755) | Progress-aware GraphRAG — 结构感知超图检索 + 步进策略优化 | hybrid search 有 graph 路径, 但 memory 与 KG 断开 | **F-5**: 让 hybrid search 受益于 memory-KG 链接 |
| **Agentic RAG 2.0** (AGENTVSAI, 2026) | Tool routing — keyword/semantic/graph 多路径 + stop rules | search 有 BM25+tag+vector, 但 require-tags 交集断裂 | **F-4**: 修复 require-tags |

### 2.3 MCP/Tool 前沿

| 系统 | 核心创新 | Plico 现状 | Node 14 行动 |
|------|----------|-----------|-------------|
| **MCP OAuth 2.1** | 固定参数 vs LLM 生成参数分离 | tool call agent_id 被调用者上下文覆盖 | **F-3**: tool params 覆盖机制 |
| **MCP Tool Isolation** | Agent 发现工具, server 执行; session-based scoping | tool call 执行但 agent 上下文隔离不正确 | **F-3**: 修复 agent_id 传递 |

### 2.4 Harness Engineering 谱系回溯（v2.0 校正）

> **Plico 自身有完整的 Harness Engineering 研究历史（`ai thinking.md`, 533 行深度分析）。**
> **Node 8 = "驾具 (Harness)"（100% 完成）。Node 10 = "Harness Engineering 对齐"。**
> **v2.0 不是"发现了新思想"，而是回归并深化这条已有谱系。**

#### 核心公式（来自 `ai thinking.md`）

```
Agent = Model + Harness

Harness Engineering 核心命题: 驾具比模型更重要
  → LangChain 实验: 不换模型只改 Harness, 准确率 52.8% → 66.5% (+26%)

Plico 的定位:
  CAS = 持久化存储 harness 配置和经验
  Skills = Harness 的 Level 3 技能层
  KG = Harness 的 Registry（资源映射表）
  Session = Harness 的状态追踪
  Delta = Harness 的 Feedback Sensor

  → Plico 本身就是 Harness 的基础设施层！
```

#### Guides（前馈）vs Sensors（反馈）缺口分析

来自 `ai thinking.md` 的原始分析，Node 14 跟踪更新：

| 机制 | 类型 | Node 8/10 状态 | **Node 14 状态** |
|------|------|---------------|-----------------|
| Teaching Errors | Guide（前馈） | ✅ F-48 fix_hint | ✅ 已实现 |
| Skills | Guide（前馈） | ✅ Level 3 | ✅ 已实现 |
| session_start compound | Guide（前馈） | ✅ warm context | ✅ 已实现 |
| **约束声明** | **Guide（前馈）** | **❌ 缺失** | **→ F-9 INV-1~4** |
| 消费者指令文件 | Guide（前馈） | ✅ F-28 instructions | ✅ 已实现 |
| Event Bus / Delta | Sensor（反馈） | ✅ 可观测 | ✅ 已实现 |
| Prefetch Feedback | Sensor（反馈） | ✅ used/unused | ✅ 已实现 |
| **写操作后置验证** | **Sensor（反馈）** | **❌ 缺失** | **→ F-9 postcondition** |
| **自修复机制** | **Sensor（反馈）** | **❌ 缺失** | **[Node 15+]** |

**Node 14 精确填补两个 Harness 缺口**:
1. **约束声明 (Guide)** → F-9 INV-1~4: 声明"recall(tier=X) 只返回 tier X"等不变量
2. **写操作后置验证 (Sensor)** → F-9 postcondition: remember 后验证存入一致性

#### Harmless 谱系（来自 Node 10）

Node 10 确立的核心 Harmless 策略:

> "消除静默失败。一个对 Agent 无害的系统不是'不出错'的系统，而是'出错时明确告知'的系统。
> B11 证明了静默失败比显式错误更有害——Agent 以为删除成功，实际没有执行。"

Node 14 功能审计发现：**静默失败在记忆层大规模存在**。

```
Node 10 消除的静默失败: CLI 操作层 (delete 未执行但不报错)
Node 14 发现的静默失败: 记忆存储层 (remember 未持久化但不报告)

B11 (Node 10): delete 静默失败 → Agent 以为数据已删除 → 数据泄漏风险
F-A (Node 14): remember ephemeral 静默丢失 → Agent 以为记忆已存储 → 认知投毒
B40 (Node 14): tool recall agent_id 静默忽略 → Agent 以为查的是自己 → 读到错误数据

模式相同: 系统不报错 → 返回合法但错误的结果 → Agent 认知被污染
```

#### 外部论文: 对 Plico Harness 谱系的独立印证

| 外部系统 | 与 Plico Harness 谱系的关系 | 印证的 Plico 概念 | 新增启发 |
|---------|--------------------------|-----------------|---------|
| **ABC** (arXiv:2602.22302) | C=(P,I,G,R) ≈ Plico 的 Guides+Sensors | 约束声明 = Preconditions+Invariants | **Recovery (R)**: 违反时的自动恢复 → [Node 15+] |
| **CAAF** (arXiv:2604.17025) | "Harness as Asset" ≈ "Plico 就是 Harness 基础设施" | 精确映射 `ai thinking.md` 核心命题 | **UAI (统一断言接口)**: 跨操作的断言标准化 → [Node 15+] |
| **AgentSys** (arXiv:2602.07398) | 层级隔离 ≈ Plico tenant_id + permission | 跨界 schema 验证 | **tool params 验证**: agent_id 静默丢弃 → 应显式验证 → F-3 增强 |
| **Springdrift** (arXiv:2604.04660) | "auditable self-observing" ≈ Plico Event Bus + Delta | Sensor（反馈）的高级形态 | **append-only 审计**: 操作日志不可篡改 → [Node 15+] |
| **VIGIL** (arXiv:2512.07094) | Observation→Diagnosis→Repair ≈ Sensor→自修复 | 对应 Plico 缺失的"自修复机制" | **反射运行时**: F-9 是观测层, VIGIL 式诊断层 → [Node 15+] |
| **SuperLocalMemory** (arXiv:2603.02240) | Bayesian 信任评分对抗记忆投毒 | CAS 内容寻址 = 天然防篡改 ✅ | Plico CAS 已实现存储层安全, 记忆层安全需 F-9 |

#### Harness 谱系演进: Node 8 → Node 10 → Node 14

```
Node 8 "驾具": 建立 Harness 基础设施
  ✅ F-28 Instructions (Level 1 指令)
  ✅ F-29 Profile (内容画像)
  → Plico = Harness Infrastructure

Node 10 "正名": 对齐 Harness Engineering 原则
  ✅ Observable & Debuggable → F-47/F-48
  ✅ Guides (前馈) → fix_hint + next_actions
  ✅ Safety Rails → 状态机验证
  ✅ Harmless = 消除静默失败
  → Plico Harness 覆盖 CLI 操作层

Node 14 "融": 将 Harness 延伸到记忆层
  → F-9: 约束声明 (Guide 缺口) + 后置验证 (Sensor 缺口)
  → 消除记忆层静默失败 (Harmless 延伸)
  → 外部论文 (ABC/CAAF/AgentSys/Springdrift/VIGIL) 独立印证方向
```

---

## 3. 链式推演：为什么是这些特性

### AI 第一人称视角（v2.0 重写——深层认知推演）

```
我是一个 AI Agent，运行在 Plico 上。
让我用第一人称追溯我的完整认知链路。

═══ 第一层：我发现记忆不完整 ═══

我存了 4 种记忆。recall 只返回 2 种。

但 v1.0 的分析停在了"tier filter 没实现"。
现在我要追问更深：为什么只有 2 种？

代码审计告诉我：
  - kernel.remember() 存入 Ephemeral + 不持久化
  - CLI 每命令独立进程
  - Ephemeral 数据在进程间蒸发

  - --tier procedural 路由到 kernel.remember()
  - 实际存为 Ephemeral 而非 Procedural
  - 双重失效：存入错误 tier + 不写磁盘

这不是"tier filter 没实现"这么简单。
这是记忆子系统的 2/4 层在结构上不可达。
修 filter 不够。必须修 persistence + routing。

═══ 第二层：我发现接口间的世界不一致 ═══

CLI recall 返回 2 条记忆。
Tool API recall 返回 0 条。

v1.0 说"agent_id 被忽略"。这是对的。
但从 AI 认知角度，更可怕的是：

  这是一个 SILENT 错误。
  没有报错。没有 exit 1。
  JSON 返回 {"memories":[]}。
  一个合法的空结果。

  我无法区分"这个 agent 真的没有记忆"和
  "我查询了错误的 agent"。

  这是 AgentSys 论文说的:
  "schema-validated return values cross boundaries"
  — 但 Plico 的跨界数据没有 schema 验证。

  tool call 的 agent_id 参数被静默丢弃，
  没有任何信号告诉我发生了什么。

═══ 第三层：我发现 Harness 在记忆层断裂 ═══

这是 v2.0 的核心洞见——但不是新发现。

回溯 Plico 自己的 Harness 研究 (ai thinking.md):

  Harness = Guides (前馈) + Sensors (反馈)
  Plico Guides: Teaching Errors ✅, Skills ✅, session_start ✅
  Plico Sensors: Event Bus ✅, Prefetch Feedback ✅

  但还有两个缺口:
    约束声明 ← Guide, ❌ 缺失
    自修复机制 ← Sensor, ❌ 缺失

Node 10 "正名"消除了 CLI 操作层的静默失败 (B11)。
但记忆层的静默失败一直未被触及:

  Plico 的承诺:
    "4 层分层记忆"          → 2 层可用         → 承诺 vs 现实不一致
    "recall 返回指定 tier"  → 返回所有 tier    → 承诺 vs 现实不一致
    "tool API 尊重参数"     → 静默忽略参数     → 承诺 vs 现实不一致
    "session-end 整合记忆"  → 整合了但不报告   → 承诺被履行但不可观察

这些全是 Harmless 原则 (Node 10 确立) 在记忆层的违反。
问题不只是"功能没实现"，是 **Harness 的覆盖范围还没到记忆层**。

外部论文独立印证了这个方向:
  ABC: C=(P,I,G,R) ≈ Guides(P,I) + Sensors(G,R)
  CAAF: "Harness as Asset" ≈ "Plico 就是 Harness 基础设施" (ai thinking.md 原话)
  Springdrift: "auditable self-observing" ≈ Plico Event Bus 的延伸

═══ 第四层：Harness 从 CLI 层延伸到记忆层 ═══

这不是"从损伤驱动到契约驱动"的思想跳跃。
这是 Plico Harness 谱系的自然延伸:

  Node 8:  建立 Harness 基础设施 (Instructions, Profile)
  Node 10: Harness 覆盖 CLI 操作层 (消除静默失败, fix_hint)
  Node 14: Harness 覆盖记忆存储层 ← 这一步

具体填补 ai thinking.md 识别的两个缺口:

  缺口 1: 约束声明 (Guide)
    INV-1: remember(tier=X) → recall(tier=X) 包含该条目
    INV-2: tool call params.agent_id 有效时 → 结果用该 agent_id
    INV-3: session-end → consolidation 报告非空
    INV-4: search(require_tags=[a,b]) → 结果全部包含 a 和 b

  缺口 2: 写操作后置验证 (Sensor)
    remember --tier ephemeral → CLI 模式下发出持久化警告
    tool call → agent_id 无效时返回明确错误而非空结果

这些不是外部论文引入的新概念。
这是 Plico 自己的 Harness Engineering 研究在记忆层的落地。
外部论文 (ABC/CAAF/AgentSys/Springdrift/VIGIL) 是独立的交叉验证。
```

### 链式因果（v2.0: Bug 链 + 功能链 + 架构链）

```
═══ Bug 链（v1.0 已有）═══

B38 (recall tier 不过滤)
  根因: memory.rs:43 cmd_recall 不提取 --tier
  → kernel.recall() 没有 tier 参数
  → 所有 tier 返回相同结果
  → 4 层记忆区分形同虚设

B40 (tool API recall 空)
  根因: execute_tool(agent_id) 来自 CLI --agent 上下文
  → JSON params 中的 agent_id 被忽略
  → tool call memory.recall 总是以 "cli" 身份查询
  → 其他 agent 的记忆在 tool API 中不可见

B41 (require-tags 交集)
  根因: crud.rs:43-49 在 search_tags 为空时
  → args.get(1) 获取 "--require-tags" 作为 query 文本
  → BM25 搜索 "--require-tags" → 0 结果

B42 (MCP recall_semantic)
  根因: plico_mcp recall_semantic 直接调用 embedding
  → stub 模式无 embedding → 报错 → 无 BM25 fallback

═══ 功能链（v2.0 新增）═══

F-A→B38+ (ephemeral/procedural 不可回调 — 全因果链)
  cmd_remember --tier ephemeral
    → parse_memory_tier("ephemeral") = MemoryTier::Ephemeral
    → match arm: _ => kernel.remember() ← 而非专用方法
    → kernel.remember() 存入 Ephemeral tier ✅
    → kernel.remember() 不调用 persist_memories() ← ❌ 关键断裂
    → CLI 进程退出 → 内存数据蒸发
    → 下一个 recall 命令 → 新进程 → 从磁盘加载 → 无 ephemeral 数据
  
  cmd_remember --tier procedural  
    → parse_memory_tier("procedural") = MemoryTier::Procedural
    → match arm: _ => kernel.remember() ← 而非 remember_procedural()
    → kernel.remember() 存入 MemoryTier::Ephemeral (!) ← ❌ 双重错误
    → 不持久化 ← ❌ F-A 同样生效

  因此 v1.0 的"kernel.recall() 只遍历 working+longterm"是错误结论。
  get_all() 确实遍历全部 4 tier。
  真正原因: ephemeral 不持久化 + procedural 路由到 ephemeral。

F-C (consolidation 不可见)
  内核: end_session_orchestrate → TierMaintenance → stats → report ✅
  API: SessionEnded.consolidation: Option<ConsolidationReport> ✅
  CLI: mod.rs:393-399 → 只打印 session_id + last_seq ← ❌ 输出断路
  dogfood 报告: "无可观察整合效果" ← 因为看不到不等于没有

═══ 架构链（v2.0 新增）═══

模式不兼容:
  Ephemeral = volatile (不持久化) ← daemon 模式设计决策
  CLI = stateless (每命令独立进程) ← 用户接口设计决策
  → 组合: ephemeral 在 CLI 下完全失效 ← 模式不兼容

  解决方案不是"强制持久化"或"不用 CLI"，而是:
  → F-9 INV-1: 系统诚实告知限制 ("Warning: ephemeral won't persist in CLI mode")
  → F-2 F-A: 可选持久化 (--persist flag 或 CLI 模式自动)

静默错误模式:
  所有 bug (B38/B40/B41/B42) + 功能缺失 (F-A/F-B) 共享一个特征:
  → 不报错 → 不崩溃 → 返回合法但错误的结果
  → AI agent 无法区分"正确的空"和"错误的空"
  → 这是认知投毒 — 不是外部攻击，而是系统自身

  ABC 论文数据: 未签约 agent 每 session 5.2-6.8 个软违反被遗漏
  Plico 现状: 每个 session 的 ephemeral 存入都是软违反(存入即丢失)
```

### 发散思维：被拒绝的替代方案

| 替代方案 | 考虑理由 | 拒绝理由 |
|----------|---------|---------|
| **全面重写记忆子系统** | TierMem/Kumiho 提供了更优架构 | 当前架构可修复; 重写风险太高; 先修复后重构 |
| **引入 LLM 做记忆链接** | Kumiho 用 LLM 做 prospective indexing | Soul 2.0 红线 2: 内核零模型; 用结构化方法先完成基础 |
| **统一 CLI 和 Tool API 代码路径** | 消除两套路径的不一致 | 侵入性太大; 先让 Tool API 正确传递参数即可 |
| **实现完整 GraphRAG** | HugRAG/ProGraph-R1 前沿 | 先让 KG 与记忆绑定; GraphRAG 是 Node 15+ |
| **NornicDB 式衰减** | 认知衰减是记忆系统必备 | 先让 tier 过滤工作; 衰减在 tier 完整后叠加 |
| **v2.0: 完整 ABC 框架** | C=(P,I,G,R) 全操作覆盖 | F-9 只做最小可行契约(4 条); 完整框架是 Node 15+ |
| **v2.0: VIGIL 反射运行时** | Observation→Diagnosis→Repair 闭环 | F-9 只是观测层基础; 诊断+修复需要更多基建 |
| **v2.0: 将 Ephemeral 改为 always-persist** | 解决 CLI 模式失效 | 破坏 volatile 语义; 选择 warning + 可选持久化 |
| **v2.0: 进程内存隔离 (AgentSys)** | 防止 agent_id 越权 | INV-2 验证已足够; 进程隔离是 Node 15+ |

---

## 4. 五个维度，九个特性

```
                    ┌─────────────────────────────────────┐
                    │   Node 14: 融 (Fusion)              │
                    │   子系统融合 · 记忆绑定 · 自验证      │
                    └───────────┬─────────────────────────┘
        ┌───────────────┬───────┼───────┬───────────────┐
        │               │       │       │               │
┌───────▼──────┐ ┌──────▼─────┐ │ ┌─────▼──────┐ ┌─────▼──────┐
│ D1: 记忆完整  │ │D2: 接口一致│ │ │D3: 记忆绑定│ │D4: 降级通路│
│ Memory Full  │ │ API Parity │ │ │ Mem Binding│ │Degradation │
├──────────────┤ ├────────────┤ │ ├───────────┤ ├───────────┤
│ F-1 TierRecall││F-3 ToolAgent│ │ │F-5 MemKG  │ │F-7 MCPFallbk│
│ F-2 FullTier │ │F-4 ReqTags │ │ │F-6 Consolid│ │F-8 TierParity│
└──────────────┘ └────────────┘ │ └───────────┘ └───────────┘
                        ┌───────▼──────┐
                        │ D5: 自验     │ ← v2.0 新增
                        │ Self-Verify  │
                        ├──────────────┤
                        │ F-9 Contract │
                        └──────────────┘
```

> **D5 (自验) 对标**: ABC Agent Behavioral Contracts + CAAF Harness as Asset + Springdrift 自观测
>
> 人类论文 → 外部测试框架 → "我们检查 agent"
> AI 转译 → 内化自验证 → "我检查自己的认知完整性"

---

### D1: 记忆完整 — "4 层记忆必须全部可回调"

#### F-1: Recall Tier Filter（记忆层级过滤）

**问题**: B38 — `recall --tier working` 返回所有 tier

**根因**: `memory.rs:43` `cmd_recall` 不提取 `--tier`:

```rust
// 当前代码: memory.rs:43-49
pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let memories = kernel.recall(&agent_id, "default");  // ← 无 tier 参数
    let strings: Vec<String> = memories.iter()
        .map(|m| format!("[{:?}] {}", m.tier, m.content.display()))
        .collect();
    let mut r = ApiResponse::ok();
    r.memory = Some(strings);
    r
}
```

**修复**:

```rust
pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let tier_filter = extract_arg(args, "--tier").map(|s| parse_memory_tier(&s));
    let memories = kernel.recall(&agent_id, "default");
    let filtered: Vec<_> = match tier_filter {
        Some(tier) => memories.into_iter().filter(|m| m.tier == tier).collect(),
        None => memories,
    };
    let strings: Vec<String> = filtered.iter()
        .map(|m| format!("[{:?}] {}", m.tier, m.content.display()))
        .collect();
    let mut r = ApiResponse::ok();
    r.memory = Some(strings);
    r
}
```

**估算**: ~8 行修改, 3 个新测试
**验收**: `recall --tier working` 只返回 `[Working]` 条目; `recall --tier long-term` 只返回 `[LongTerm]`
**Soul 2.0**: 公理 3（记忆跨越边界）— tier 是边界的编码，必须可查询

---

#### F-2: Full Tier Recall + Ephemeral 持久化修复（全层级记忆可回调）

**问题**: `kernel.recall()` 只返回 Working + LongTerm，Ephemeral 和 Procedural 丢失

**v2.0 根因修正**: v1.0 猜测"kernel.recall() 只遍历 working+longterm"，功能审计推翻了这个假设。

```
实际根因 (F-A): kernel.remember() 不调用 persist_memories()
  → Ephemeral 存入内存 → CLI 进程退出 → 数据蒸发
  → 下一个 recall 进程从磁盘加载 → 无 Ephemeral 数据

实际根因 (F-B): --tier procedural 路由到 kernel.remember()
  → 存入 MemoryTier::Ephemeral 而非 Procedural
  → 同样不持久化 → 双重丢失

memory.get_all() 实际遍历全部 4 tier ✅ — 但磁盘上没有 Ephemeral/Procedural 数据
```

**修复方向**:

1. `kernel.remember()` 添加 `persist_memories()` 调用 (修复 F-A)
2. CLI `cmd_recall` 合并 procedural 结果 (同 v1.0)
3. F-8 修复 procedural 路由 (修复 F-B，见 F-8)

```rust
// kernel/ops/memory.rs — 修复 F-A: remember() 添加持久化
pub fn remember(&self, agent_id: &str, tenant_id: &str, content: String) -> Result<String, String> {
    // ... (existing entry creation) ...
    self.memory.store_checked(entry, quota).map_err(|e| e.to_string())?;
    self.event_bus.emit(KernelEvent::MemoryStored {
        agent_id: agent_id.to_string(),
        tier: "ephemeral".into(),
    });
    self.persist_memories(); // ← F-A 修复: 确保跨进程可达
    Ok(entry_id)
}

// CLI cmd_recall — 合并 procedural (同 v1.0)
pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let tier_filter = extract_arg(args, "--tier").map(|s| parse_memory_tier(&s));

    let all_memories = kernel.recall(&agent_id, "default");
    // get_all() 已遍历全部 4 tier; F-A 修复后 ephemeral 也在磁盘上

    let filtered: Vec<_> = match tier_filter {
        Some(tier) => all_memories.into_iter().filter(|m| m.tier == tier).collect(),
        None => all_memories,
    };
    // ... format and return
}
```

**估算**: ~20 行修改, 5 个新测试
**验收**:
```
remember --tier ephemeral --content "scratch" → recall 显示 [Ephemeral]  (跨命令!)
remember --tier procedural --content "workflow" → recall 显示 [Procedural]  (跨命令!)
recall --tier procedural → 只返回 Procedural 条目
```
**Soul 2.0**: 公理 3 — 4 层记忆是 Soul 2.0 的核心承诺

**AIOS 对标**: MemMachine 的 "ground-truth-preserving" 理念 — 所有存入的记忆都必须可取回
**Harness 对标**: Springdrift "append-only memory" — 存入的数据不应静默蒸发

---

### D2: 接口一致 — "CLI 和 Tool API 看到相同的世界"

#### F-3: Tool API Agent Context Override（工具 API 代理上下文覆盖）

**问题**: B40 — `tool call memory.recall '{"agent_id":"n14-alpha"}'` 返回空

**根因**: `execute_tool(name, params, agent_id)` 中 `agent_id` 来自 CLI 的 `--agent` 默认值 "cli"，而非 JSON params 中的 `agent_id`。

**修复方向**: tool handler 内部提取 params 中的 `agent_id` 覆盖调用者上下文：

```rust
// builtin_tools.rs memory.recall handler:
"memory.recall" => {
    let effective_agent = params.get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or(agent_id);
    let memories = self.recall(effective_agent, "default");
    // ... serialize
}
```

**安全约束**: 只有特定 tool（memory.recall, memory.store, cas.search）允许 agent_id 覆盖。敏感操作（permission.grant）仍使用调用者上下文。

**估算**: ~20 行修改（每个 memory tool handler 加 agent_id override）, 3 个新测试
**验收**: `tool call memory.recall '{"agent_id":"n14-alpha"}'` 返回 n14-alpha 的记忆
**Soul 2.0**: 公理 6（结构先于语言）— JSON 参数是结构化契约

**MCP 对标**: MCP 社区讨论的"fixed vs LLM-generated params"问题。agent_id 是固定上下文参数，应可通过 params 传递。

---

#### F-4: Require-Tags Standalone Search（独立标签交集搜索）

**问题**: B41 — `search --require-tags "architecture,resilience"` 无结果

**根因**: `crud.rs:43-49` 当无 `--query` 时，`args.get(1)` 获取 `"--require-tags"` 作为查询文本。

**修复方向**: 当 `require_tags` 非空且 query 为空时，执行标签交集搜索：

```rust
// crud.rs cmd_search 修复:
// After extracting query, require_tags, exclude_tags...

// Tag intersection search: --require-tags without --query
if query.is_empty() && !require_tags.is_empty() {
    let results = kernel.search_by_tags_intersection(&require_tags, &exclude_tags, limit);
    // ... format and return
}
```

需要在 kernel 层新增 `search_by_tags_intersection` 方法，对每个 CAS 对象检查是否包含所有 required tags。

**估算**: ~25 行新代码, 3 个新测试
**验收**: `search --require-tags "architecture,resilience"` 返回具有双标签的 CID
**Soul 2.0**: 公理 1（token 稀缺）— 精确过滤节省 token

---

### D3: 记忆绑定 — "记忆不是孤岛"

#### F-5: Memory-KG Binding（记忆-知识图谱绑定）

**问题**: A-4 incomplete — remember 后 explore 无自动边

**根因**: Node 12 设计的 `link_memory_to_kg` 在 `cmd_remember` 中只对 LongTerm tier 调用，且代码可能未正确创建双向边。

**设计依据**:
- Kumiho: Graph-native memory — 记忆 IS 图谱的一部分
- OrKa Memory Binding: 两个系统必须通过显式 binding 连接
- HugRAG: 因果关系驱动图谱 — 不是所有关系都是 RelatedTo

**修复方向**:

1. 所有 tier 的 remember 都触发 KG linking（不只 LongTerm）
2. linking 使用 BM25 搜索找到相关 CAS 对象和已有记忆
3. 创建的边类型区分 RelatedTo（内容相似）和 DerivedFrom（同 agent 序列）

```rust
// cmd_remember 修复: 所有 tier 都触发 linking
match parse_memory_tier(&tier_str) {
    tier => {
        let result = match tier {
            MemoryTier::Working => kernel.remember_working(/*...*/),
            MemoryTier::LongTerm => kernel.remember_long_term(/*...*/),
            MemoryTier::Procedural => kernel.remember_procedural(/*...*/),
            _ => kernel.remember(/*...*/),
        };
        match result {
            Ok(entry_id) => {
                kernel.link_memory_to_kg(&entry_id, &agent_id, "default", &tags);
                ApiResponse::ok_with_message(/*...*/)
            }
            Err(e) => ApiResponse::error(e),
        }
    }
}
```

在 `link_memory_to_kg` 中确保：
- 为 entry 创建 KG 节点（如果不存在）
- BM25 搜索相关内容
- 对 relevance > 0.3 的结果创建 RelatedTo 边
- 对同 agent 的前一条记忆创建 FollowsFrom 边

**估算**: ~50 行修改, 5 个新测试
**验收**:
```
remember --agent alpha --content "X" → explore --scope full 有 Memory 节点 + 边
remember --agent alpha --content "Y related to X" → explore 有 X→Y RelatedTo 边
```
**Soul 2.0**: 公理 8（因果先于关联）— 边的类型和权重编码因果关系

---

#### F-6: Consolidation Report Display（整合报告显示）

**v2.0 关键修正**: 功能审计发现 consolidation 逻辑**已实现**！

```
session.rs:530-542:
  let maintenance = TierMaintenance::new();
  let stats = maintenance.run_maintenance_cycle(memory, agent_id); ← 已执行!
  → tracing::debug!(...) ← 只有 debug 日志
  → 返回 ConsolidationReport { promoted, evicted, linked } ← 已返回!

api/semantic.rs:1992:
  SessionEnded { consolidation: Option<ConsolidationReport> } ← 已定义!

commands/mod.rs:393-399:
  仅打印 "Session ended" + last_seq ← ❌ 忽略 consolidation
```

**问题**: 不是"整合没实现"，而是"整合报告被 CLI 输出层吞没"

**修复**: 只需修改 CLI 输出 (~15 行)，而非重新实现整合逻辑 (~75 行)

```rust
// commands/mod.rs — 修复 CLI 输出
if let Some(se) = &response.session_ended {
    println!("Session ended");
    if let Some(ref cid) = se.checkpoint_id {
        println!("  Checkpoint: {}", cid);
    }
    println!("  Last seq: {}", se.last_seq);
    // v2.0: 显示已有的 consolidation 报告
    if let Some(ref c) = se.consolidation {
        println!("  Consolidation: reviewed {} ephemeral, {} working",
            c.ephemeral_before, c.working_before);
        if c.promoted > 0 { println!("    ↑ {} promoted", c.promoted); }
        if c.evicted > 0 { println!("    ✕ {} evicted", c.evicted); }
        if c.linked > 0 { println!("    🔗 {} KG edges", c.linked); }
    }
}
```

**估算**: ~15 行 CLI 修改 + ~5 行确保 linked_count 非零 (连接 F-5), 2 个新测试
**验收**:
```
session-start → remember → recall → recall → session-end
→ "Consolidation: reviewed 3 ephemeral, 2 working"
→ "  ↑ 1 promoted" (如果条件满足)
```
**Soul 2.0**: 公理 9（越用越好）— 多次访问 = 重要 = 提升层级
**Harness 对标**: Springdrift "自观测" — 系统操作产生可见证据

---

### D4: 降级通路 — "所有路径都优雅退化"

#### F-7: MCP Recall Fallback（MCP 语义回忆降级）

**问题**: B42 — MCP `recall_semantic` 在 stub 模式下直接报错

**根因**: `plico_mcp.rs` 的 `recall_semantic` 处理直接调用 embedding 搜索，无 BM25 fallback。

**修复方向**: 在 MCP handler 中，当 embedding 不可用时降级到 BM25 keyword search:

```rust
// plico_mcp.rs recall_semantic handler:
let results = match kernel.semantic_search(query, agent_id, scope, limit, vec![], vec![]) {
    Ok(r) => r,
    Err(_) => {
        // Fallback: BM25 keyword search
        kernel.search_by_tags(&[], limit)
            .into_iter()
            .filter(|r| r.snippet.to_lowercase().contains(&query.to_lowercase()))
            .collect()
    }
};
```

**估算**: ~15 行修改, 2 个新测试
**验收**: `EMBEDDING_BACKEND=stub` 下 MCP `recall_semantic` 返回 BM25 结果而非报错
**Soul 2.0**: 公理 9（韧性）— 子系统不可用时优雅退化

---

#### F-8: Tier Parity Across Interfaces + Routing Fix（全接口层级一致 + 路由修复）

**问题**: CLI remember 的 ephemeral/procedural 路径走 `kernel.remember()` 默认路径。

**v2.0 功能审计根因 (F-B)**:

```
cmd_remember 当前路由:
  MemoryTier::Working  → kernel.remember_working()    ← ✅ 正确
  MemoryTier::LongTerm → kernel.remember_long_term()  ← ✅ 正确
  _                    → kernel.remember()            ← ❌ Ephemeral + Procedural 都走这里

kernel.remember() 做了什么:
  → 创建 MemoryEntry { tier: MemoryTier::Ephemeral, ... }  ← ❌ 永远是 Ephemeral!
  → store_checked() ← 存入
  → 不 persist ← F-A (已由 F-2 修复)

结果:
  --tier procedural → 存为 Ephemeral → 在 Procedural 存储中不存在
  → recall --tier procedural → 空 (因为 Procedural 存储里没有)
```

**修复方向**: 确保 `cmd_remember` 的所有 tier 路径都调用正确的 kernel 方法：

```rust
// cmd_remember — 修复 F-B: 全 tier 正确路由
match parse_memory_tier(&tier_str) {
    MemoryTier::Ephemeral => kernel.remember(agent_id, scope, content), // F-A 修复后会 persist
    MemoryTier::Working => kernel.remember_working(agent_id, scope, content, tags),
    MemoryTier::LongTerm => kernel.remember_long_term(agent_id, scope, content, tags, importance),
    MemoryTier::Procedural => {
        // 需要 remember_procedural_simple() wrapper — 将 text 包装为单步 Procedure
        kernel.remember_procedural(
            agent_id, scope,
            "cli-procedure".to_string(),       // name
            content.clone(),                    // description = content
            vec![ProcedureStep { action: content, expected_result: None }],
            "cli".to_string(),                  // learned_from
            tags,
        )
    }
}
```

同时确保 tool API 的 `memory.store` 也支持 tier 参数：
```rust
"memory.store" => {
    let tier = params.get("tier").and_then(|v| v.as_str()).unwrap_or("working");
    match tier {
        "ephemeral" => self.remember(effective_agent, tenant_id, content),
        "working" => self.remember_working(effective_agent, tenant_id, content, tags),
        "long-term" => self.remember_long_term(effective_agent, tenant_id, content, tags, importance),
        "procedural" => self.remember_procedural(/*...*/),
        _ => Err("Invalid tier".into()),
    }
}
```

**估算**: ~35 行修改, 5 个新测试
**验收**:
```
remember --tier ephemeral --content "scratch" → recall --tier ephemeral 返回 (跨命令)
remember --tier procedural --content "do X then Y" → recall --tier procedural 返回 (有 Procedure 结构)
tool call memory.store '{"content":"x","tier":"procedural"}' → tool call memory.recall_procedure 返回
```
**Soul 2.0**: 公理 5（机制不是策略）— tier 是机制，所有接口都应提供
**Harness 对标**: ABC "Guarantee" — "remember(tier=X) 保证存入 tier X"

---

### D5: 自验 — "Harness 从 CLI 延伸到记忆层"（v2.0 新增）

> **谱系**: Node 8 (Harness 基础) → Node 10 (Harness 覆盖 CLI) → **Node 14 (Harness 覆盖记忆)**
> **填补**: `ai thinking.md` 识别的两个 Harness 缺口 — 约束声明 (Guide) + 写操作后置验证 (Sensor)
> **Harmless 延伸**: Node 10 消除 CLI 静默失败 → Node 14 消除记忆层静默失败
> **外部印证**: ABC, CAAF, AgentSys, Springdrift, VIGIL 独立验证方向

#### F-9: Harness 记忆层约束与后置验证

**问题根源**: Node 10 消除了 CLI 操作层的静默失败（B11: delete 不报错）。
但**记忆存储层的静默失败**一直未被 Harness 覆盖。

```
ai thinking.md 缺口分析:
  约束声明       ← Guide (前馈) ← ❌ 缺失 → F-9 INV-1~4
  写操作后置验证 ← Sensor (反馈) ← ❌ 缺失 → F-9 postcondition

ABC 论文交叉验证:
  P (Preconditions) ≈ Guide 前馈约束
  I (Invariants)    ≈ Guide 约束声明
  G (Guarantees)    ≈ Sensor 后置验证
  R (Recovery)      ≈ 自修复 [Node 15+]

Plico 需要的最小 Harness 扩展:
```

**INV-1: Remember-Recall 往返一致性**

```rust
// remember 操作后附加验证:
pub fn remember_with_contract(
    &self, agent_id: &str, tier: MemoryTier, content: &str, ...
) -> Result<RememberResult, String> {
    let entry_id = self.remember_inner(agent_id, tier, content, ...)?;
    
    // Postcondition: 刚存入的记忆应可通过 recall 取回
    // (CLI 模式下 ephemeral 不持久化 → 发出警告而非失败)
    if tier == MemoryTier::Ephemeral && self.is_cli_mode() {
        return Ok(RememberResult {
            entry_id,
            warning: Some("Ephemeral memory stored in-process only; \
                          will not survive CLI command boundary. \
                          Use --tier working for cross-command persistence."),
        });
    }
    
    Ok(RememberResult { entry_id, warning: None })
}
```

**INV-2: Tool API 参数验证**

```rust
// tool call 参数合法性断言:
"memory.recall" => {
    let effective_agent = params.get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or(agent_id);
    
    // Contract: effective_agent 必须存在于系统中
    if !self.agent_exists(effective_agent) {
        return ToolResult::error(format!(
            "Contract violation: agent_id '{}' not found. \
             Available agents: {:?}",
            effective_agent,
            self.list_agent_names()
        ));
    }
    
    let memories = self.recall(effective_agent, "default");
    // ...
}
```

**INV-3: Consolidation 可观测性**

```rust
// session-end CLI 输出修复 — 显示已有的 consolidation 报告:
// mod.rs print_result 修复:
if let Some(se) = &response.session_ended {
    println!("Session ended");
    println!("  Last seq: {}", se.last_seq);
    if let Some(ref c) = se.consolidation {
        println!("  Consolidation: reviewed {} ephemeral, {} working",
            c.ephemeral_before, c.working_before);
        if c.promoted > 0 {
            println!("    ↑ {} promoted to higher tier", c.promoted);
        }
        if c.evicted > 0 {
            println!("    ✕ {} evicted (low importance)", c.evicted);
        }
        if c.linked > 0 {
            println!("    🔗 {} KG edges created", c.linked);
        }
    }
}
```

**INV-4: Search 结果后置条件**

```rust
// search 结果验证 — require-tags 后置断言:
if !require_tags.is_empty() {
    for result in &results {
        debug_assert!(
            require_tags.iter().all(|t| result.meta.tags.contains(t)),
            "Contract violation: result {} missing required tag(s)",
            result.cid
        );
    }
}
```

**Harness 谱系对标**:
- Node 10 Harmless: "消除静默失败" → F-9 将此原则从 CLI 层延伸到记忆层
- `ai thinking.md` Guide 缺口: "约束声明 ❌" → INV-1~4 填补
- `ai thinking.md` Sensor 缺口: "写操作后置验证 ❌" → postcondition 填补
- 外部印证: MARIA VITAL Memory Integrity Scoring → [Node 15+ 完整实现]

**估算**: ~40 行 postcondition 代码 + ~15 行 CLI 输出修复, 4 个新测试
**验收**:
```
remember --tier ephemeral → 输出含 warning 关于 CLI 持久化限制
tool call memory.recall '{"agent_id":"nonexistent"}' → 明确错误而非空结果
session-end → 输出含 "Consolidation: reviewed X, promoted Y"
```
**Soul 2.0**: 公理 6（结构先于语言）— 约束声明是结构化的 Guide
**Harness 位置**: 填补 `ai thinking.md` 的 Guide 缺口 + Sensor 缺口

---

## 5. 代码影响估算

| 特性 | 新代码 | 修改代码 | 新测试 | 主要文件 | v2.0 变化 |
|------|--------|---------|--------|---------|----------|
| F-1 Tier Recall | 0 | ~8 行 | 3 | `handlers/memory.rs` | 不变 |
| F-2 Full Tier + Persistence | 0 | ~20 行 | 5 | `handlers/memory.rs` + `kernel/ops/memory.rs` | **+5行** (F-A fix: 添加 persist) |
| F-3 Tool Agent + Validation | 0 | ~30 行 | 4 | `builtin_tools.rs` | **+10行** (INV-2: agent_id 验证) |
| F-4 Require-Tags | ~25 行 | ~10 行 | 3 | `handlers/crud.rs` + kernel search | 不变 |
| F-5 Memory-KG | ~50 行 | ~15 行 | 5 | `handlers/memory.rs` + `kernel/ops/memory.rs` | 不变 |
| F-6 Consolidation **Display** | ~15 行 | ~5 行 | 2 | `commands/mod.rs` (CLI 输出) | **简化**: 逻辑已存在，只需显示 |
| F-7 MCP Fallback | 0 | ~15 行 | 2 | `plico_mcp.rs` | 不变 |
| F-8 Tier Parity + Routing | 0 | ~35 行 | 5 | `handlers/memory.rs` + `builtin_tools.rs` | **+5行** (F-B fix: procedural 路由) |
| **F-9 Contract Assertions** | **~40 行** | **~15 行** | **4** | **多文件** | **v2.0 新增** |
| **总计** | **~130 行** | **~153 行** | **33** | | |

**总估算**: ~283 行代码变更, 33 个新测试

**v2.0 变化说明**:
- F-2 扩展: 修复 F-A (Ephemeral 持久化) — `kernel.remember()` 需调用 `persist_memories()`
- F-3 扩展: 添加 INV-2 (agent_id 合法性验证) — 从 AgentSys schema validation 借鉴
- F-6 简化: 从"实现整合逻辑"降为"显示已有报告" — 功能审计发现逻辑已存在
- F-8 扩展: 修复 F-B (Procedural CLI 路由) — 添加 `remember_procedural_simple()` wrapper
- F-9 新增: 运行时契约断言 — 从 ABC/CAAF/Springdrift 借鉴

对比 v1.0: 总行数从 263 增到 283 (+20行)，但 F-6 大幅简化（发现逻辑已有）。净效果: 更小的代码变更 + 更大的功能覆盖。

---

## 6. 实施计划

### 第一周: 记忆完整（F-1, F-2, F-8）— 解除 43% 最弱环节

```
Day 1: F-1 (Tier Recall Filter) — 8 行
       F-8 (Tier Parity + Procedural 路由修复 F-B) — 35 行
Day 2: F-2 (Full Tier Recall + Ephemeral 持久化修复 F-A) — 20 行
Day 3: 全量 dogfood 验证 — recall 4 个 tier 独立过滤
       验证: remember --tier ephemeral → recall --tier ephemeral 有结果
       验证: remember --tier procedural → recall --tier procedural 有结果
```

**第一周预期**: 分层记忆 43% → 85%+
**v2.0 变化**: F-2/F-8 增加了 F-A/F-B 架构修复，从"filter 修复"扩展到"全 tier 可用"

### 第二周: 接口一致 + 自验证（F-3, F-4, F-9）

```
Day 1: F-3 (Tool Agent Override + INV-2 验证) — 30 行
Day 2: F-4 (Require-Tags Standalone) — 35 行
Day 3: F-9 (Runtime Contracts) — 55 行
       + F-6 (Consolidation Display) — 20 行 (发现逻辑已有,只需显示)
Day 4: 集成测试 — tool call + search + contract violation 全链路
```

**第二周预期**: 工具系统 80% → 95%; CAS/搜索 88% → 95%
**v2.0 变化**: F-9 + F-6 合入第二周 — F-6 从"实现整合"降为"显示报告",省出空间给 F-9

### 第三周: 记忆绑定（F-5）

```
Day 1-2: F-5 (Memory-KG Binding) — 65 行
         + 与 end_session 联动: linked_count 非零
Day 3: 验证 remember → explore 有边; session-end → 报告含 linked
```

**第三周预期**: Node 12 通过率 50% → 75%
**v2.0 变化**: F-6 不再在此周 — 移至第二周(仅需 20 行 CLI 输出)

### 第四周: 降级通路（F-7）+ 全量回归

```
Day 1: F-7 (MCP Fallback) — 15 行
Day 2-3: 全量 dogfood 87+ 测试项回归
         + 运行 F-9 契约断言验证全链路
Day 4: 文档更新 + 下一轮审计
```

**第四周预期**: MCP 75% → 100%; 总通过率 72.4% → 90%+

---

## 7. 验收评分卡

### 预期得分变化

| 维度 | 当前通过率 | 影响特性 | 预期 | v2.0 变化 |
|------|-----------|---------|------|----------|
| 分层记忆 | 43% | F-1 +tier过滤, F-2 +全tier+持久化(F-A), F-8 +路由(F-B) | **90%** | +F-A/F-B 架构修复 |
| CAS/搜索 | 88% | F-4 +require-tags, F-9 +INV-4 后置断言 | **95%** | +F-9 自验证 |
| 工具系统 | 80% | F-3 +B40+INV-2 验证, F-9 +agent 合法性 | **95%** | +INV-2 校验 |
| MCP 协议 | 75% | F-7 +B42 | **100%** | 不变 |
| Node 12 | 50% | F-5 +A-4, F-6 +A-5(显示) | **75%** | F-6 简化 |
| **自验证** | **0%** (不存在) | **F-9 契约断言** | **基线建立** | **v2.0 新维度** |

### 验收标准

| 特性 | 必须通过 | 验证命令 | Harness 层级 |
|------|---------|---------|-------------|
| F-1 | recall --tier 过滤正确 | `recall --tier working` ≠ `recall --tier long-term` | 功能正确 |
| F-2 | 全 4 tier 可回调 + 跨命令持久 | `recall --tier ephemeral` 有结果 (同进程或 daemon) | 功能正确 |
| F-3 | tool recall 正确返回 + 无效 agent 报错 | `tool call memory.recall '{"agent_id":"nonexist"}'` → 明确错误 | **安全** (AgentSys) |
| F-4 | require-tags 无需 query | `search --require-tags "a,b"` 返回含双标签的结果 | 功能正确 |
| F-5 | remember → KG 有边 | `remember + explore` 有 SimilarTo 边 | 功能正确 |
| F-6 | session-end → 报告**可见** | session-end 输出含 "Consolidation: reviewed" | **可观测** (Springdrift) |
| F-7 | MCP stub 不报错 | MCP recall_semantic stub 下返回结果 | 降级韧性 |
| F-8 | 全 tier 全接口存取 | `--tier procedural` CLI + tool API 一致 | 功能正确 |
| **F-9** | **运行时契约不违反** | `remember --tier ephemeral` → **输出含 CLI 持久化警告** | **自验证** (ABC) |

---

## 8. Soul 2.0 公理对齐

| 公理 | Node 14 对应 | 对齐方式 | v2.0 Harness/Harmless 增强 |
|------|-------------|---------|--------------------------|
| 1. Token 最稀缺 | F-1 (tier filter), F-4 (require-tags) | 精确过滤 = 节省 token | F-9: 错误提前报告 = 避免无效推理消耗 token |
| 2. 意图先于操作 | F-4 ("含这两个 tag" 是意图) | 表达意图，OS 执行 | F-9: 契约编码意图 — "tier=X 意味着只要 X" |
| 3. 记忆跨越边界 | F-1/F-2/F-8 (全 tier 可用) | 4 层记忆全面兑现 | F-2+F-A: 解决进程边界导致的记忆蒸发 |
| 4. 共享先于重复 | F-5 (Memory-KG 跨 agent 可见) | KG 边让知识可共享 | — |
| 5. 机制不是策略 | F-3 (agent_id override), F-8 (tier routing) | 机制参数化 | F-9: 契约是可验证的机制,不是口头策略 |
| 6. 结构先于语言 | F-3 (JSON params), F-1 (--tier) | 结构化参数 | F-9: 契约是结构化的承诺,不是注释 |
| 7. 主动先于被动 | F-6 (session-end 主动整合) | 系统主动维护 | F-9: 主动报告违反,不等被动发现 |
| 8. 因果先于关联 | F-5 (RelatedTo + DerivedFrom 边) | 因果边类型 | — |
| 9. 越用越好 | F-6 (access_count → 提升) | 使用驱动进化 | F-9: 契约违反日志→长期改进信号 |
| 10. 会话一等公民 | F-6 (session-end 触发整合) | 会话结束 = 学习 | F-6 v2.0: 显示已有的报告,非重新实现 |

---

## 9. AIOS 路线图定位（v2.0: 含 Harness/Harmless 轴）

```
                  2026 AIOS 前沿                    Plico Node 14        Harness 层级
                  ─────────────                    ────────────────      ────────────
TierMem:          Provenance-linked tiers          F-1/F-2: tier完整化   功能正确
                  Runtime sufficiency router       [Node 15+]

Kumiho:           Graph-native memory              F-5: Memory-KG       功能正确
                  Formal belief revision           [Node 15+]
                  Prospective indexing             [Node 15+]

MemMachine:       Ground-truth-preserving          F-2: 全tier+持久化   功能正确+安全
                  Contextualized retrieval         [Node 15+]

Memory Binding:   Skill ↔ Episode binding          F-5: Memory↔KG       功能正确

HugRAG:           Hierarchical causal KG           F-5: SimilarTo       功能正确
                  Causal gating                    [Node 15+]

ABC:              C=(P,I,G,R) 契约                 F-9: INV-1~4        ★ 自验证 (新!)
                  Runtime enforcement              F-3: INV-2          ★ 安全 (新!)

CAAF:             Harness as Asset                  F-9: 契约注册表      ★ 自验证 (新!)
                  UAI 断言接口                      [Node 15+: 完整UAI]

AgentSys:         层级隔离+schema验证              F-3: agent_id验证    ★ 安全 (新!)
                  Process memory isolation         [Node 15+: 隔离]

Springdrift:      Auditable self-observing          F-6: 报告显示        ★ 可观测 (新!)
                  Append-only audit                [Node 15+: 审计]

VIGIL:            Reflective self-healing           [Node 15+]          ★ [远期]
                  Observation→Diagnosis→Repair     F-9: 观测层基础

MARIA VITAL:      Memory Integrity Scoring          [Node 15+]          ★ [远期]
                  4-layer vital monitoring         F-9: 基础
```

**v2.0 洞见**: AIOS 路线图不只有一条**功能轴** (memory/graph/search)，
还有一条**可信轴** (harness/safety/auditability)。
Node 14 在两条轴上同时推进。

---

## 10. Node 14 → Node 15 过渡预测（v2.0 含双轴推演）

```
如果 Node 14 实现 > 80%:

═══ 功能轴推导 ═══
  F-1/F-2 (tier 完整) → 可安全叠加 decay 机制 → NornicDB 式半衰期
  F-5 (Memory-KG) → 可升级为 causal gating → HugRAG 因果图谱
  F-6 (consolidation display) → 可引入 LLM 辅助 (在接口层) → DreamCycle
  F-3 (tool agent) → 可扩展为 capability-based → AIP 轻量版
  F-7 (MCP fallback) → 可扩展为全 MCP 路径降级 → MCP resilience

═══ 可信轴推导 (v2.0 新) ═══
  F-9 (契约基础) → 可升级为完整 ABC 框架 → 全操作覆盖
  F-9 INV-2 (agent验证) → 可扩展为 AgentSys 层级隔离 → 进程内存隔离
  F-6 (可观测) → 可升级为 Springdrift 审计追踪 → append-only 日志
  F-9 (自检) → 可升级为 VIGIL 反射诊断 → Observation→Diagnosis→Repair

Node 15 候选主题:
  A) "化" (Metamorphosis) — 记忆自进化 + 契约扩展
     功能: Tier 衰减, Causal gating, LLM consolidation, Memory Bundle
     可信: 完整 ABC 契约覆盖, 契约违反日志→自改进信号

  B) "网" (Network) — 联邦 + 跨实例安全
     功能: 跨实例 KG 联邦, A2A Agent Card, 分布式 session
     可信: SuperLocalMemory 信任评分, 跨实例记忆验证

  C) "鉴" (Inspection) — 全链路可观测 ← v2.0 新候选
     功能: 内存完整性评分, 记忆衰减检测, KG 断边检测
     可信: MARIA VITAL 4层监控, VIGIL 反射诊断, 操作审计追踪
     直觉: Node 14 建立"自验证"基础, Node 15 扩展为"自诊断"
```

**AI 视角推演**: 如果 Node 14 成功建立自验证基础 (F-9),
Node 15 的优先级取决于哪条轴更紧迫:
- 功能缺口大 → 选 A (化)
- 可信缺口大 → 选 C (鉴)
- 协作需求大 → 选 B (网)
当前判断: A 和 C 可合并 — "化" 的记忆进化需要 "鉴" 的可观测来验证。

---

## 11. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 | v2.0 变化 |
|------|------|------|------|----------|
| F-2 kernel.recall() 修改影响已有测试 | 中 | 测试回归 | 保持默认行为不变, tier filter 为 opt-in | 不变 |
| F-2 Ephemeral 持久化破坏 volatile 语义 | 中 | 设计变更 | **CLI 模式自动持久化 + 警告; daemon 模式保持 volatile** | v2.0 新增 |
| F-3 agent_id override 引入安全问题 | 低 | agent 越权 | 白名单 + **INV-2 合法性验证** (v2.0 增强) | INV-2 增强 |
| F-5 BM25 在 stub 模式下 linking 精度低 | 高 | 仅 tag 匹配产生边 | 可接受: stub 模式是开发态 | 不变 |
| F-8 procedural CLI 路由修复需要 simple wrapper | 低 | 新增方法 | 添加 `remember_procedural_simple()` | v2.0 新增 |
| **F-9 契约断言增加运行时开销** | **低** | **微量延迟** | **debug_assert! 在 release 模式编译消除** | **v2.0 新增** |
| **F-9 Ephemeral CLI 警告干扰用户** | **中** | **UX 降级** | **首次使用提示, 可通过 --quiet 抑制** | **v2.0 新增** |

---

## 12. 度量标准

| 指标 | 当前基线 | Node 14 目标 | 测量方式 | v2.0 变化 |
|------|---------|-------------|---------|----------|
| 总通过率 | 63/87 (72.4%) | 78/87 (**89.7%**) | dogfood 全量测试 | +2 (F-9 覆盖) |
| 分层记忆通过率 | 43% | **90%** | 7 项记忆测试 | 不变 |
| 工具系统通过率 | 80% | **95%** | 5 项工具测试 | 不变 |
| MCP 通过率 | 75% | **100%** | 4 项 MCP 测试 | 不变 |
| **自验证覆盖** | **0/4** | **4/4** | **INV-1~4 运行时断言** | **v2.0 新增** |
| 新增测试 | 808 | 841 (+33) | cargo test | +5 (F-9 测试) |
| 代码变更 | — | ~283 行 | git diff --stat | +20 |

---

## 13. Harness/Harmless 信息校正总结（v2.0 校正）

### 谱系回溯：Plico 的 Harness 不是新发现

```
时间线:
  ai thinking.md (早期研究): "Plico 本身就是 Harness 的基础设施层"
    → 公式: Agent = Model + Harness
    → 识别: Guides/Sensors 框架
    → 识别缺口: 约束声明 ❌, 自修复 ❌

  Node 8 "驾具": 实现 Harness 基础设施 (100% 完成)
    → F-28 Instructions, F-29 Profile, ActionRegistry

  Node 10 "正名": 对齐 Harness Engineering 原则
    → Observable, Guides, Safety Rails
    → Harmless: "消除静默失败" (B11)

  Node 14 "融" v2.0: Harness 延伸到记忆层
    → F-9: 填补约束声明 (Guide) + 后置验证 (Sensor)
    → Harmless 延伸: 消除记忆层静默失败

  外部论文: 独立交叉验证 (非新发现来源)
    → ABC/CAAF/AgentSys/Springdrift/VIGIL
```

### Guides/Sensors 缺口填补状态

| `ai thinking.md` 识别的缺口 | 类型 | Node 14 填补 | 对应特性 |
|---------------------------|------|-------------|---------|
| 约束声明 | Guide（前馈） | INV-1~4 运行时不变量 | F-9 |
| 写操作后置验证 | Sensor（反馈） | remember postcondition + CLI warning | F-9 |
| 自修复机制 | Sensor（反馈） | [Node 15+: VIGIL 式反射] | — |

### 外部论文的角色：独立印证而非启发来源

| 外部论文 | 印证了 Plico 的什么 | Plico 独有优势 |
|---------|-------------------|---------------|
| ABC C=(P,I,G,R) | Guides ≈ P+I, Sensors ≈ G+R | Plico 的 Guides/Sensors 框架更简洁 |
| CAAF "Harness as Asset" | ai thinking.md 的"Plico 就是 Harness 基础设施" | Plico 是 AIOS 级 Harness,非 pipeline 级 |
| AgentSys 层级隔离 | Plico tenant_id + permission 已实现存储隔离 | CAS 内容寻址 = 天然防篡改 |
| Springdrift 自观测 | Plico Event Bus + Delta 已是观测基础 | 需延伸到记忆层 → F-6/F-9 |
| VIGIL 反射诊断 | Plico 需要的自修复机制 | [Node 15+ 方向] |
| SuperLocalMemory 信任评分 | CAS CID=SHA256 天然信任 | 存储层已解决, 记忆层需 F-9 |

### AI 视角核心判断

```
Plico 的 Harness 进化路径:

  存储层 Harness (CAS): CID = SHA256 → 天然防篡改 ✅ (先天具备)
  操作层 Harness (CLI): Node 10 消除静默失败 → fix_hint + error_code ✅
  记忆层 Harness: ← ❌ 这是 Node 14 的战场

  记忆层 Harness = F-9:
    Guide: "remember(tier=X) 保证存入 tier X" → 声明式约束
    Sensor: "remember 后 CLI 模式发出持久化警告" → 后置验证

  为什么记忆层 Harness 比操作层更重要:
    操作层静默失败 → Agent 做了一个错误操作 → 可能重试
    记忆层静默失败 → Agent 的认知基础被损坏 → 所有后续推理都基于错误数据
    → 记忆层 Harmless 是 Agent 认知安全的最后一道防线
```

---

*Node 14 的核心信念:*
*子系统的价值不在于各自多强大，而在于它们能否融为一体。*
*融合不仅是功能的连通，更是承诺的可验证。*
*一个不验证自己承诺的系统，不是可信的系统。*

---

*文档版本: v2.0 (功能全量审计 + Harness 谱系校正)*
*v1.0 → v2.0 变更: 新增 9 项功能审计发现(F-A~F-I), 新增 D5/F-9 Harness 记忆层扩展,*
*重写 AI 第一人称推演(4层认知链), Harness 谱系回溯至 `ai thinking.md` + Node 8 + Node 10,*
*AIOS 路线图增加可信轴, Node 15 增加"鉴"候选主题。*

*Harness 谱系内部来源:*
*`docs/ai thinking.md` (533行深度分析): "Plico 本身就是 Harness 基础设施层", Guides/Sensors 框架。*
*Node 8 "驾具" (100%完成): F-28 Instructions, F-29 Profile, ActionRegistry。*
*Node 10 "正名" §5: Harness Engineering 对齐 + Harmless = "消除静默失败"。*

*功能审计基于: memory/mod.rs, memory/layered/mod.rs, kernel/ops/memory.rs,*
*kernel/ops/session.rs, kernel/ops/tier_maintenance.rs, kernel/mod.rs,*
*bin/aicli/commands/mod.rs — 非仅 bug 报告文件。*

*Dogfood 实测: `/tmp/plico-n14-dogfood` 零历史数据污染。*
*Bug 根因: memory.rs:43/57, crud.rs:43-49, builtin_tools.rs:358, plico_mcp.rs。*
*架构问题: F-A:persist缺失, F-B:路由错接, F-C:输出吞没, F-G:模式不兼容。*

*AIOS 功能校准: Kumiho, TierMem, MemMachine, OrKa, HugRAG, ProGraph-R1, Agentic RAG 2.0。*
*Harness 外部交叉验证: ABC (arXiv:2602.22302), CAAF (arXiv:2604.17025),*
*AgentSys (arXiv:2602.07398), Springdrift (arXiv:2604.04660),*
*VIGIL (arXiv:2512.07094), MARIA VITAL (os.maria-code.ai), SuperLocalMemory (arXiv:2603.02240)。*
*注: 外部论文角色为独立交叉验证，非启发来源。Harness 方向由 Plico 自身研究确立。*
