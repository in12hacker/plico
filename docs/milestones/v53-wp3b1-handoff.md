# v53 WP3B.1：Single-Writer Append & Idempotent Receipt

**日期**：2026-08-18  
**状态**：READY FOR ARCHITECTURE；DEVELOPMENT BLOCKED  
**当前基线**：`c65ea2ca54a8cddc41744a2ea92ba4007aae4fdd`  
**预期验收**：R4（local-only；GitHub 仅承载 branch/commit/tag/PR）

## 1. 责任归属

| 阶段 | 唯一责任方 | 可以做什么 | 完成定义 |
|---|---|---|---|
| WP3B.1-A Contract Freeze | **外包架构组** | Accepted ADR、exact API/scope、clock/lock/receipt 合同、外部反例、local verifier | 发布 architecture base/tag；开发组收到 exact SHA |
| WP3B.1-B Implementation | **开发组** | 只在冻结 scope 内实现 facade/reducer/clock/tests | 提交 candidate SHA 和 self-evidence；不得宣称 R4 GO |
| R4 Adversarial Acceptance | **Plico 架构组/安全审计** | 独立 mutation、并发、crash、restart、scope 与累计回归 | acceptance commit/tag；明确 GO/NO-GO |

在 WP3B.1-A 完成前，开发组不得修改 append/store/CAS，也不得根据本文件猜测接口开工。

## 2. 前置证据

- R2 durable structural store：`16e610629d3741f8e7cedf1b471e974c81960cb6`；
- R3 deterministic readonly facade：`3e2a7c20076638ab8e632090d7761b651559c8ea`；
- R3.1 portable MCP harness：`932a7d42779bf3da37c0235c865cba8f10b8cf14`；
- R3.1.1 managed child lifecycle：`c65ea2ca54a8cddc41744a2ea92ba4007aae4fdd`。

R3.1.1 的 RAII 回收债 `D-MCP-1` 视为关闭。它不授权 WP3B 修改 MCP、public protocol、kernel 或 scheduler。

## 3. 唯一目标

在已接纳的 structural store 与唯一 deterministic reducer 之上增加 crate-private fixture facade：

1. append Started；
2. append Terminal；
3. 相同 canonical request retry 返回首次 receipt，零持久化变化；
4. 不同 retry 返回 typed conflict；
5. facade 内单写者线性化；
6. writer clock 单调且 JSON-safe；
7. restart 后 Open/Terminal/read receipt 与首次提交逐字段一致。

receipt 只证明本地 `unverified_fixture` ledger commit，不证明真实执行、外部副作用、可信身份、evidence
存在性/授权或 VEG。

## 4. WP3B.1-A：外包架构组任务

外包架构组必须先交付：

1. 新 Accepted ADR（建议 `ADR-0010: Execution Observation Single-Writer Append v1`）；
2. 绑定本文件前置 SHA 的 checkpoint/spec；
3. exact production API、visibility、allowed paths 和 forbidden paths；
4. single-writer lock 顺序、poison、candidate 与 restart 状态机；
5. writer clock 与 test-only clock seam；
6. request→event→segment→view→root→pointer→receipt 的唯一推导顺序；
7. architecture-owned external mutation/concurrency/crash corpus；
8. local-only scope verifier 与执行命令；
9. architecture base commit + lightweight tag。

建议冻结的 facade 形状如下；它只是待 A 阶段确认的输入，不是开发授权：

```rust
pub(crate) struct FixtureObservationLedgerV1 { /* sealed */ }

impl FixtureObservationLedgerV1 {
    pub(crate) fn open_fixture(
        vault: Arc<PersonalVaultStorage>,
    ) -> Result<Self, ObservationStoreError>;

    pub(crate) fn append_started(
        &self,
        request: AppendStartedRequestV1,
    ) -> Result<ObservationReceiptV1, ObservationStoreError>;

    pub(crate) fn append_terminal(
        &self,
        request: AppendTerminalRequestV1,
    ) -> Result<ObservationReceiptV1, ObservationStoreError>;

    pub(crate) fn read_attempt(
        &self,
        key: &ExecutionAttemptKeyV1,
    ) -> Result<Option<FixtureAttemptObservationV1>, ObservationStoreError>;
}
```

架构组不得通过扩大通用 CAS/Memory API 来实现 facade，也不得把 fixture writer 接入生产生命周期。

## 5. WP3B.1-B：开发组任务

收到 WP3B.1-A 的 exact SHA/tag 后，开发组只实现冻结接口。预计文件形状：

```text
src/memory/execution_observation/mod.rs
src/memory/execution_observation/store/mod.rs
src/memory/execution_observation/store/facade.rs
src/memory/execution_observation/store/reducer.rs
src/memory/execution_observation/store/clock.rs
src/memory/execution_observation/store/facade/tests/**
```

最终 allowed paths 以 A 阶段 spec 为准。需要修改 CAS、model/hash/error、Cargo、kernel、scheduler、API、MCP、bin
时立即停止并提交 Architecture Deviation。

## 6. 根因级不变量

- 一个 facade mutex 覆盖：poison check → head equality → transition/idempotency → clock → bundle → commit → state update；
- 固定锁序：facade state → structural store；锁内禁止外部 callback；
- startup、append planning、current-view derivation、receipt reconstruction 必须使用同一个 reducer；
- 幂等命中必须发生在读取 clock、分配 sequence、构造 object 之前；
- receipt 只能由 accepted event/root identity 重建，不能信任 caller、candidate、orphan 或未验证 cache；
- pre-exchange 失败不更新内存状态、不返回 receipt；
- `CommitIndeterminate` 后当前 handle 的 read/write 全部 `Poisoned`；reopen 只相信 authoritative active；
- same Terminal 并发返回同一首次 receipt；different Terminal 只有一个成功；
- candidate 不得被 facade 自动 promote；
- `recorded_at_ms = max(system_now_ms, previous_accepted_recorded_at_ms)`，且不得超过 JSON-safe integer；
- default-off 生命周期、public exact-14、MCP/aicli 与 vault tree 保持不变。

## 7. R4 最低反例矩阵

| 类别 | 必须证明 |
|---|---|
| Started idempotency | 相同 request 返回首次 receipt；clock/root/candidate/inventory 零变化 |
| Started conflict | 不同或并发 Started 只接受一个，另一方 typed conflict |
| Terminal idempotency | 相同 Terminal 并发获得相同 receipt |
| Terminal conflict | outcome/evidence/policy/runtime rebind 拒绝且零 mutation |
| Restart | Open/Terminal 与两个 receipt 逐字段相同 |
| Outcomes | success/failure/timeout/cancel/indeterminate 五类重启一致 |
| Crash | pre-exchange 可重试；post-exchange uncertainty poison；reopen 按 active reconcile |
| Candidate | 合法 child 也只验证不 promote；非法 transition/view fail closed |
| Clock | rollback、同毫秒、溢出、JSON-safe 边界、幂等不消费 clock |
| Concurrency | 不产生 sibling accepted roots、双 Terminal 或 receipt 漂移 |
| Boundary | 无 kernel/scheduler/public/MCP wiring；default-off 零资源变化 |

## 8. 滚动债务

| ID | 内容 | 本里程碑处理 | Owner | 最迟关闭 |
|---|---|---|---|---|
| D-MCP-1 | child 未 wait/reap | 已由 `c65ea2c` 关闭 | 外包架构组 | CLOSED |
| D-MCP-2 | inherent `McpClient::call_tool` 对 poisoned mutex 仍可能 panic | 不阻止 WP3B 开工；禁止开发组跨域修 | 外包架构组 | 下一个 transport hardening 里程碑 |
| D-MCP-3 | MCP request/initialize 没有 I/O deadline | 不阻止 WP3B 开工；禁止在 WP3B 加临时 timeout | 外包架构组 | 下一个 transport hardening 里程碑 |

债务滚动不等于静默允许：必须保留 ID、owner、反例和关闭里程碑。

## 9. 成本控制与执行顺序

1. static scope/API 检查；
2. reducer、idempotency、clock 定向测试；
3. fault/concurrency/restart 外部语料；
4. fmt、diff-check、Clippy；
5. 以上全绿后只运行一次全库 lib；
6. 最后执行 public catalog/default-off differential。

任一 P0 或真值/权限/持久化 P1 出现即停止。局部且跨域独立的问题进入上表，不为每个小问题新开里程碑。

## 10. 非目标

- kernel/scheduler producer admission；
- public API、MCP、aicli 或 daemon 接线；
- action/tool replay、外部副作用、compensation；
- trusted identity/evidence authorization；
- fixture→trusted promotion；
- branch/checkpoint/rollback、retention、repair、migration；
- Memory/KG/vector/summary/skill/feedback/learning。

## 11. 交付格式

外包架构组交付：ADR/spec/verifier/corpus 的 exact SHA、tag、allowed paths、命令和限制。  
开发组交付：candidate SHA、exact diff、定向原始摘要、已知限制和 Architecture Deviation（如有）。  
只有 Plico 架构组可以发布 R4 GO。
