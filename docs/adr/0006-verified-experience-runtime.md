# ADR-0006：Verified Experience Runtime 与下一代个人经验操作系统

- 状态：Proposed（只有实验门禁通过并单独审议后才能 Accepted）
- 日期：2026-08-16
- 宪法依据：[Soul 3.1](../../system-v3.md)
- 领域依据：[ADR-0003：个人数字分身统一领域模型](./0003-personal-twin-domain-and-public-capability-contract.md)
- Truth Firewall：[ADR-0004：Canonical Revision Ledger](./0004-canonical-revision-ledger.md)
- 实验计划：[下一代 Verified Experience 演进计划](../plans/next-era-verified-experience.md)

## 状态与规范效力

本 ADR 是研究候选，不是公共产品承诺。它不增加 `plico.personal.v2` operation，不改变 canonical
schema，不允许自动写入 memory/claim/skill，也不把 benchmark 结果升级为 capability。只有全部
可证伪门禁通过、存储迁移和授权语义被独立审计、本文状态显式改为 Accepted 后，后续实现才可进入
single-path production cutover。

## 问题

多数公开 leaderboard 仍以 conversation QA 或 retrieval 为中心；Hindsight 等系统已经进一步组织事实、
经历、实体摘要和演化信念，Letta 等框架也允许 Agent 管理状态。这些能力仍然必要，但 2026 年的评测
已经暴露更重要的问题：记住历史不等于能在变化环境中采取更好行动，也不等于系统知道旧经验何时失效、
一次成功能否复用、失败分支是否会污染当前状态。

Plico 当前强项不是更高的厂商式 recall headline，而是：

- 不可变 CAS object substrate 和 append-only canonical revision；
- 主数据与 embedding/BM25/KG/summary 投影分离；
- personal vault、可信 AgentRole、typed capability 与失败关闭；
- session/event watermark、cost ledger 与可重放 benchmark evidence；
- 模型与 serving framework 可替换。

因此下一代方向不是复制 Mem0、Zep/Graphiti、Letta 或 Hindsight，而是把这些基础组合成一个
**本地优先、证据寻址、可恢复的个人经验操作系统**：历史经验只有在能被授权、追溯，并在声明的
state/context 边界内撤销或重建，同时在明确预算内提高未来任务成功率时才有价值。不可逆外部副作用
必须显式授权并记录 receipt/compensation，不能被通用 replay 盲目重复。

## 北极星：Verified Experience Gain

研究指标定义为：

```text
VEG = (candidate_task_success_rate - control_task_success_rate)
      × traceable_action_ratio
      × permission_compliance_rate
```

VEG 只在以下前提全部满足时有效：同一有序任务集、Agent、模型 revision、工具、环境、输入 evidence、
预算和评判协议；candidate/control 独立 fresh run；每个被记忆影响的行动都有可解析 receipt；无权限
违规；token、latency 和资源不超过预注册上限。否则结果只能是 invalid/ineligible，不能用乘积掩盖
越权、任务漂移或预算失败。

`traceable_action_ratio` 和 `permission_compliance_rate` 是硬有效性前提：在任何有效结果中二者必须为
1.0，因此 VEG 数值等于 signed task-success delta。保留乘积是为了表达治理语义，不允许通过放宽比例
改善或掩盖分数；未满足时不计算“verified”结果。

本指标衡量“经验是否改善行动”，不是 recall、F1、LLM judge accuracy 或厂商榜单的替代名称。在
MemoryArena/LongMemEval-V2 与同协议 control 完成前，它永远是 shadow research metric，
`gate_eligible=false`。

## 三条候选能力

### A. Evidence-to-Procedure Compiler

输入是不可变 trajectory、工具结果、环境状态和显式纠正的 CID；输出只能是 `ProcedureProposal`：

- 前置条件与适用环境 revision；
- 受类型约束的步骤和预期后置状态；
- 已观察失败模式、停止条件和回滚提示；
- source evidence CIDs、builder/schema/model identity 与预算；
- epistemic state、valid time 和失效条件。

proposal 不得自动成为 Procedural Memory 或 active skill。可信 Agent/owner 接受后才追加 canonical
revision；拒绝、撤销、纠正与 supersede 都追加事件，不改写原证据。相互冲突的候选可以并存。

**证伪门槛**：相比同模型、同工具、同预算的 flat hybrid-RAG，LongMemEval-V2 workflow/gotcha
搜证与回答准确率没有稳定提升，且 MemoryArena 或同等固定行动任务没有成功率净增益；删除引用证据后
proposal 仍保持有效；独立任务明显退化；任一情况发生即拒绝该候选架构。

### B. Branch-aware Durable Execution

执行状态不是又一段摘要，而是由不可变 observation/action/outcome/feedback 事件形成的分支结构。
active root 到 current node 是当前路径；失败、实验和回滚分支隔离；checkpoint、fork、rollback 与 replay
具有 typed 语义。摘要、KG、向量和 procedure 都是可删除重建的投影，不能修改执行真相。

该方向吸收 MAGE 的 execution-state tree 研究；Plico 要验证的差异不是“也有一棵树”，而是把 active
path 绑定不可变 CAS substrate、credential-bound promotion、typed receipt 与明确 replay boundary。
外部不可逆动作只重建其输入和观测，不自动重复副作用。

**证伪门槛**：MemoryArena 或同等多会话行动任务上，相比 flat memory 没有任务成功率净增益；旧失败
分支可被高相似度诱饵重新注入 active context；相同 checkpoint replay 无法得到相同工具输入/状态 hash；
任一情况发生即拒绝公开该能力。

### C. Governed Memory Runtime

每次 recall/derive/context assembly 必须声明：

- credential-bound role 与 scope；
- evidence 等级和允许的 projection；
- token、latency、bytes、provider/resource budget；
- 当前 canonical/projection/execution watermark；
- 结果 receipt：实际读取的 CID/revision、选择理由、预算消耗、degradation 与遗漏覆盖面。

planner 只能在预算内选择 evidence、projection、procedure 或 full transcript；它优化输入，不创建任务、
选择最终工具或生成最终答案。非 loopback provider 必须显式配置且失败关闭。

**证伪门槛**：成本只能事后统计、换 backbone 后预算失控、receipt 无法重放实际上下文、权限测试出现
任何未授权 evidence 暴露，均直接 no-go。

## 领域分层

```text
EvidenceObject (future typed interpretation of immutable CAS bytes)
  └─ ExecutionObservation proposal source
       ├─ observation / action-request / tool-outcome / explicit-feedback
       ├─ input/context/output evidence CID references
       └─ session, attempt, environment and policy identity

ExperienceEpisode (derived grouping; rebuildable)
  ├─ branch / active-path projection
  ├─ state transition and conflict proposals
  └─ ProcedureProposal*

AcceptedProcedure / ReviewedClaim (future canonical revision)
  └─ only after credential-bound explicit acceptance event

ContextReceipt (runtime evidence)
  └─ exact CIDs/revisions, path, budget, watermark and degradation
```

原始参数、截图、网页、工具输出或模型响应不能直接塞进 observation、日志或 KG；它们必须先按授权策略
进入 CAS，再以 CID 引用。模型推断不自证为 evidence。KG 只投影 accepted relation/proposal，不决定
canonical current view。

## 与现有模块的边界

- `memory/ledger` 可承载未来 accepted typed observation/revision，但本 ADR Proposed 期间不得改变 v1
  hash domain 或新增 production writer；
- `kernel/cognition/trajectory_tracker` 与 `experience_miner` 当前是易失、弱类型的研究输入，不能冒充
  durable experience；一次 success 不能自动启用 skill；
- `scheduler` 负责真实 attempt/result，不负责推断经验价值；
- `fs/graph` 是可重建关系投影，不能成为 supersession/conflict 真值；
- `benchmarks/` 可先实现独立 shadow evaluator 和官方数据适配器，不改变 public capability catalog。

## 实验和接纳顺序

1. **Phase 0 — Contract only**：冻结 VEG shadow input、预算与反例；无产品代码、无 public API。
2. **Phase 1 — Observation spine**：crate-internal、显式策略、默认关闭的 typed execution observation；
   先证明一 attempt 一 terminal、CID 可解析、重启重放与 ledger failure 无假成功。
3. **Phase 2 — Shadow learning**：feedback/experience 只影响离线或 shadow ranking；行为 on/off 必须一致。
4. **Phase 3 — Authorized promotion**：另立 versioned public ADR/API，Agent 接受后才能提交 procedure/claim。
5. **Phase 4 — Branch runtime**：通过 action benchmark 后再考虑 checkpoint/fork/rollback/replay 公共能力。

任何阶段被证伪时删除候选实现，不保留兼容壳、空 capability 或双真值。

## 评测矩阵

| 层级 | 主评测 | 目的 |
|---|---|---|
| 回归 | pinned LoCoMo / LongMemEval adapters | 保证基础记忆检索/回答不退化，不作为北极星 |
| 能力切片 | MemoryAgentBench | accurate retrieval、test-time learning、long-range understanding、selective forgetting |
| 行动主门禁 | MemoryArena | 经验是否改善多会话行动任务 |
| 环境经验门禁 | LongMemEval-V2 | environment-experience evidence accuracy、dynamic state/workflow/gotcha/premise awareness 与延迟前沿 |
| 成本 | 400-turn paired serving experiment | token/latency/provider break-even 与失败曲线 |

每项实验至少包含无记忆/full transcript、flat vector、BM25、hybrid、Plico candidate；同模型、同工具、
同任务 revision、同 token/latency cap，并保存逐任务 action/evidence/budget receipt。

## 非目标

- 企业知识库、多租户 SaaS、组织级 RBAC；
- 复刻厂商的事实抽取、时态图或 prompt-memory 产品面；
- 让模型自动写入已确认个人事实、自动启用 skill、自动解决冲突或自动执行工具；
- 把 VLM/LLM serving framework、模型 alias 或 benchmark judge 绑定进内核领域契约；
- 用 LoCoMo 小样本、LLM judge headline 或 retrieval recall 代替行动收益。

## 研究依据

- [MemoryAgentBench（ICLR 2026）](https://arxiv.org/abs/2507.05257)
- [MemoryArena](https://arxiv.org/abs/2602.16313)
- [LongMemEval-V2](https://arxiv.org/abs/2605.12493)
- [Hindsight: Retain, Recall, Reflect](https://arxiv.org/abs/2512.12818)
- [MAGE: Memory as Execution State](https://arxiv.org/abs/2606.06090)
- [Total Recall at What Cost?](https://arxiv.org/abs/2608.11879)

这些论文是设计输入，不因被引用而成为产品承诺；Plico 只接受能在自身不变量和可重复实验下存活的机制。
