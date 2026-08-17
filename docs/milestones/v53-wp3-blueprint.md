# v53 WP3 Blueprint：Validated Attempt Facade & Deterministic State Reconstruction

**日期**：2026-08-17
**状态**：R2 accepted；WP3A read-only slice 由 ADR-0009 授权；WP3B 仍为 Draft
**前置条件**：R2 acceptance `16e610629d3741f8e7cedf1b471e974c81960cb6`
**规范依据**：[ADR-0007](../adr/0007-execution-observation-ledger-v1.md)、
[ADR-0008](../adr/0008-execution-observation-store-substrate-v1.md) 与
[WP2-R2 checkpoint](./v53-wp2-r2-checkpoint.md)

本蓝图用于在 R2 关闭后生成正式 WP3 ADR/checkpoint/spec/verifier。它不是实施授权，不冻结 commit、digest、scope、
API surface 或 tag；第三方开发组不得据此开工。候选 C2 `f60eec1` 当前为 R2 NO-GO，因此不能作为 WP3 base。

## 1. 唯一目标

在已接纳的 sealed structural store 之上增加一个 crate-private fixture facade，用同一 deterministic reducer：

- 从 authoritative active chain 重建 attempt state 与 current view；
- 验证 stored current-view hash 与逐事件推导一致；
- 实现 Started/Terminal append、幂等、conflict 和 single-writer linearization；
- 从已验证 event/root chain 重建 `ObservationReceiptV1`；
- 重启后保持 Open/Terminal 与首次 receipt 完全一致。

这里的 replay 仅是 observation integrity replay，不是 ADR-0006 的 action/tool replay。receipt 只证明本地 fixture
ledger commit，不证明真实执行、外部副作用、identity、evidence existence/readability/authorization 或 Verified
Experience Gain。

## 2. 建议的新规范载体

R2 acceptance 后创建窄化的 `ADR-0009: Execution Observation Verified Facade and Replay v1`，冻结 structural
store→facade 唯一调用边界、single-writer lock、candidate semantic validation、writer clock 和 test-only seams。

ADR-0006 继续保持 Proposed；ADR-0009 不接受 Branch Runtime、public capability 或生产 producer。

## 3. 预期 exact production API

正式 checkpoint 只能在 R2 后重新核对并冻结以下 ADR-0007 surface：

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

不得增加 list/history/export/raw read/raw writer/recovery/promote/path/config constructor。test-only clock/fault seam 必须
`#[cfg(test)]` 且不进入 production surface。

## 4. 根因式内部结构

唯一 reducer 同时服务 startup replay、append planning、current-view derivation 和 receipt reconstruction：

```text
verified chain metadata
  → bounded chronological event load
  → attempt transition classifier
  → deterministic sorted current view
  → view hash/root binding check
  → verified in-memory state
```

状态分类只定义一次：Started 为 `Absent | Same | Different`，Terminal 为
`NoStarted | Open | Same | Different`。caller boundary 将 Same 映射为幂等 receipt、Different 映射 typed conflict；
stored boundary 将重复或非法 transition 映射为 `CorruptStore`。不得维护 startup/append/read 三套状态机。

## 5. Replay 的有界实现

先从 active root 逆向收集最多 20,000 条小型 metadata：root/event/segment/view digest、generation、sequence、
kind 与 committed time；再反转 metadata，逐个有界加载 event 并应用 reducer。不得把最多 20,000 个 128 KiB
request 或完整历史 view 同时保存在内存。

attempt state 只保留后续验证需要的 request/event digest、policy/runtime digest、evidence counts、receipt 和 attempt
view。每一步重建 current-view JCS hash并与对应 root 绑定比较；receipt 必须来自 accepted event/root identity，
不能从 candidate、orphan、caller 预填字段或未验证 cache 构造。

## 6. Single-writer 与 cache 规则

facade 使用一个 `Mutex<VerifiedLedgerStateV1>` 覆盖：poison check → head equality → transition/idempotency → time
allocation → bundle construction → structural commit → verified state update。`read_attempt` 使用同一把锁。

- 固定锁序为 facade state → structural store；
- 不在锁内调用外部 callback；
- mutex poison typed fail closed，不 panic；
- pre-exchange 失败不更新 cache；
- `CommitIndeterminate` 后所有 facade read/write 返回 `Poisoned`；
- reopen 只从 authoritative active 全量重建，不返回不确定调用的预构造 receipt。

## 7. Candidate 与 genesis

- `P(G0)/E` 只接受 exact G0；`P/E` 且 generation > 0 继续 `CorruptStore`，不 repair；
- `P(Rn)/P(Rn-1)` replay active，candidate 仅是 active chain 中已验证旧 parent；
- `P(Rn)/P(Rn+1)` 在 active state clone 上验证 candidate event/view/transition，随后丢弃 clone，绝不 promote；
- unaccepted candidate 不影响 read、receipt、writer clock 或 idempotency；retry 可以覆盖它；
- `E/P(G0)` 仍是唯一重算 exact genesis 后重试正常 publish 的恢复特例。

## 8. Writer time 与幂等

新 accepted event 使用：

```text
recorded_at_ms = max(system_now_ms, previous_accepted_recorded_at_ms)
root.committed_at_ms = recorded_at_ms
```

replay 验证 root/event time 相等且 accepted event time 不下降。时间只取 active chain；candidate/orphan time 不参与。
SystemTime 早于 epoch、转换溢出或超过 JSON-safe integer 必须在写入前 typed fail closed。

相同 accepted canonical request retry 必须在读取 clock、分配 sequence 或写任何 object 之前返回首次 receipt；root
generation、candidate bytes 和 object inventory 均不变。pre-exchange 失败没有 accepted receipt，后续 retry 可以重新
取时间。`CommitIndeterminate` 绝不返回 success/receipt。

## 9. 预期 developer scope（未冻结）

R2 后由正式 spec 给出 exact paths；当前建议仅为：

```text
src/memory/execution_observation/mod.rs          # exact re-export anchor
src/memory/execution_observation/store/mod.rs    # exact module/re-export anchors
src/memory/execution_observation/store/facade.rs
src/memory/execution_observation/store/replay.rs
src/memory/execution_observation/store/reducer.rs
src/memory/execution_observation/store/clock.rs
src/memory/execution_observation/store/facade/tests/**
```

WP1 model/hash/validation/error、WP2.1 CAS/loader/publisher/slots、Cargo/lock、kernel/scheduler/API/bin/MCP 均应由
architecture base 冻结。scope gate 必须证明 `commit_structural` 和 structural `open_fixture` 各只有 facade 一个
production callsite；test 与 architecture overlay 另行识别。

## 10. 预期 R3 验收矩阵

| 类别 | 必须证明 |
|---|---|
| F02 | Started/Terminal 同 request 返回首次 receipt；clock/root/candidate/inventory 不变 |
| F03 | 不同或并发 Started 只接受一个，其余 typed conflict |
| F04 | terminal outcome/evidence/policy/runtime rebind conflict，零 mutation |
| F05 | 相同 terminal 并发同 receipt；不同 terminal 仅一个成功，无双 head |
| F07 | Started 重启仍 Open，无自动 terminal，receipt/view 相同 |
| F08 | 五种 outcome 分别完成 Terminal restart equality |
| F09 | post-exchange uncertainty poison；reopen 只按 active reconcile |
| Stored replay | duplicate Started/Terminal、terminal-without-start、semantic view mismatch fail closed |
| Candidate | 结构合法但 transition/view 非法的 prepared child使 open fail closed且不 promote |
| Clock | rollback、同毫秒、JSON-safe 上界、幂等不消费 clock |
| Boundary | default lifecycle 零 namespace/thread/handle；无 public/live wiring |

还必须累计运行 WP1、WP2.1 architecture corpus及 privacy、scope、fmt/check/clippy gates。candidate 自测不能单独
构成 R3 evidence。

## 11. 非目标

- action/tool replay、side-effect receipt 或 compensation；
- open-attempt recovery terminal；
- trusted identity/evidence verdict、CID authorization；
- public reader/list/history/export；
- kernel/scheduler producer 或 production auto-open；
- branch/checkpoint/rollback；
- fixture→trusted promotion；
- Memory/KG/vector/skill/feedback/learning；
- authenticated anti-rollback、retention、repair、migration。

## 12. 解锁条件

只有以下全部成立才能把本蓝图转为正式 WP3 freeze：WP2.1 七项根因不变量关闭；C2.1 通过独立 R2 corpus；
R2 acceptance commit 已存在；新 ADR/checkpoint/spec/verifier 绑定其 SHA/tree；B3 不含 approval；A3 是 B3 的
approval-only direct child并有新 lightweight tag。在此之前，本文件始终保持 Draft/Blocked。
