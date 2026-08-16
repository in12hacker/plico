# 下一代 Verified Experience 演进计划

- 日期：2026-08-16
- 状态：Active research plan；不代表公共 capability
- 宪法：[Soul 3.1](../../system-v3.md)
- 候选架构：[ADR-0006](../adr/0006-verified-experience-runtime.md)

## 一句话方向

把 Plico 从“能可靠保存和召回个人记忆的内核”演化为“能让历史经验在明确权限和预算内改善未来
行动、且每次影响都可追溯，并能在声明的 state/context 边界内重建的个人经验操作系统”。

## 宽度优先原则

本计划先建立外层合同和最小类型，再逐步增加深度。每一阶段都必须能独立运行、明确报告 unsupported，
并且可以在证伪后完整删除。不得同时实现存储、学习、冲突裁决、公共 API 和 serving 优化。

## Phase 0：实验框架（本轮）

交付：

- Soul 3.1 授权学习边界；
- Proposed ADR-0006；
- benchmark-only `Verified Experience Gain` shadow evaluator；
- 对非法任务重绑定、越权、无 receipt、预算超限和非有限数值的 fail-closed 测试。

明确不交付：committed artifact serializer/deep verifier、runtime observation writer、canonical schema 变更、
public API、自动 skill/claim、产品 gate。纯函数返回值不是可发布证据，调用方修改或序列化它不会获得
任何 release/benchmark artifact 身份。

退出门槛：纯 evaluator 可重复、输入合同严格、`gate_eligible=false` 固定，现有 benchmark/Rust 门禁不退化。

## Phase 1：Execution Observation 证据脊柱

只在 ADR-0006 通过 Phase 0 审核后开始。

第三方实施合同见 [v53 Execution Observation Ledger Core](../milestones/v53-execution-observation-spine.md)。
v53 当前仍处于 Architecture Review；独立 durable namespace/writer 必须先由窄化的 Accepted ADR 授权，
开发组不得把 Proposed ADR 或本计划本身解释为开工许可。v53 只覆盖 Phase 1A 的 ledger core；下列
credential-bound producer、真实 evidence verifier 与运行时接线属于后续 Phase 1B，不得提前实现。

### Phase 1A：Ledger core（v53）

- `execution_id`、attempt、closed origin、optional future-bound identity refs；
- operation、typed outcome/error category；
- writer-stamped record time、optional execution elapsed assertion；
- input/context/output evidence CID；
- runtime/policy version。

行为：crate-internal、无 production wiring、不开 public operation。普通 kernel/daemon/config 路径不构造
writer、不 claim namespace；只有 ledger 测试显式打开。record 只验证 canonical-form CID 引用，不宣称
evidence 存在、可读或已授权。

门禁：success/failure/timeout/cancel/indeterminate schema；一 attempt 最多一个 terminal；Open 可跨重启保留；
幂等、JCS/hash stable、ledger fail 无半提交、日志无正文泄漏。Phase 1A 不把 Open 自动解释为业务失败。

### Phase 1B：Trusted producer（后续独立里程碑）

- credential-bound role、真实 intent/origin、commit-time evidence existence/readability/authorization；
- 单一 admitted execution boundary，durable begin-before-action，typed terminal 与 open-attempt recovery policy；
- 只有在该边界内，才要求每个已接受的真实 attempt 最终恰好一个 terminal observation。

## Phase 2：Feedback 与 Procedure Proposal shadow

- 显式 feedback 绑定 observation/evidence revision；冲突反馈并存；
- procedure compiler 只生成 proposal；
- proposal 只能改变 shadow score/explanation，不能改变生产召回、current view 或 active skill；
- prefetch/experience cache 必须可从 canonical observation 重建。

门禁：shadow on/off 对生产输出字节一致；引用不存在或跨 role fail closed；证据删除/失效能使候选失效；
poisoning 有界；proposal precision/recall、unused-context rate 和跨轮方差完整记录。

## Phase 3：同协议横向与授权提升

先跑同一 harness：

- no memory / full transcript；
- BM25-only / vector-only / hybrid / KG-on；
- 固定版本、adapter 和配置的 Mem0 OSS、Graphiti、Hindsight、LangMem/Mastra recipes；
- Plico proposal shadow。

固定 dataset revision、reader/judge、工具、top-k、token/latency budget、模型字节、硬件和五个 fresh run。
只有任务成功率净增益稳定且 evidence/permission/receipt 门禁全绿，才另立 Accepted ADR 与版本化 API，
让 Agent/owner 显式接受 procedure/claim。

## Phase 4：Branch-aware Durable Execution

在 MemoryArena 证明 action success 收益、LongMemEval-V2 证明 environment-experience evidence/latency
收益后，才实现 active branch、checkpoint、fork、rollback 和有声明边界的 deterministic state reconstruction。
失败分支不得被相似度召回重新注入 active context；摘要/KG 均为投影；外部副作用不得盲目重放。

## 基准优先级

1. pinned full LoCoMo 与 LongMemEval adapters（精确 source URL、revision、hash）：协议兼容和基础回归；
2. MemoryAgentBench：四能力宽度；
3. MemoryArena：行动收益主门禁；
4. LongMemEval-V2：环境经验和 latency-quality frontier；
5. 400-turn paired cost：确定何时相对 full transcript 真正 break even。

## 推理运行时独立路线

Serving 性能不与 memory quality 混因果：Ollama/Qwen3 embedding 保持可靠控制组；TensorRT-LLM 后接
vLLM 做同 checkpoint、同输入、C=1/4/16 的吞吐/延迟/失败率/资源 A/B。VLM 只进入真实 image-bearing
任务，不进入纯文本默认路径。换 runtime 不能自动升级 Verified Experience 能力。

## 停止条件

- 只提高 recall/F1、不能提高后续任务成功率；
- procedure 无法反链原始 evidence；
- 关闭候选仍会改变生产行为；
- 预算或 provider 改变后收益消失且无法解释；
- 任一未授权 evidence 泄漏；
- 需要让 KG/摘要/model output 成为第二真值源。

触发任一条件时回退到最后一个已验证阶段，并删除失败候选，不保留兼容壳。
