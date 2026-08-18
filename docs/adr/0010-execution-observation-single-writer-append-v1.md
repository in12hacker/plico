# ADR-0010: Execution Observation Single-Writer Append v1

- 状态：Accepted（v53 WP3B.1-A 契约冻结）
- 日期：2026-08-18
- 基线：`c5387448d29454a16a1647d40ba95798f3e1bab5`（handoff）及其前置
  `c65ea2c`（R3.1.1）/ `932a7d4`（R3.1）/ `3e2a7c2`（R3）/ `16e6106`（R2 structural store）
- 决策方：Plico 架构组（WP3B.1-A，外包架构组执行）
- 影响：`src/memory/execution_observation/`（新增 facade 层）；不触碰 CAS/model/hash/error 的既有冻结面

## 1. 背景与问题

ADR-0008 交付了 durable structural store（`FixtureObservationStoreV1`：单事务 mutex、
typestate loader、dual-slot publish），ADR-0009 交付了唯一 deterministic readonly facade
（`FixtureObservationReaderV1`：existing-only 闭包读 + 独立 replay 验证）。二者之间缺一个
**写侧编排层**：调用方目前没有受控方式提交 Started/Terminal，也没有 receipt 语义。
WP3B.1 冻结该编排层。它不新增存储原语、不新增 reducer、不接线生产生命周期。

## 2. 决策

新增 crate-private fixture facade `FixtureObservationLedgerV1`，位于
`src/memory/execution_observation/store/facade.rs`，冻结 API 逐字如下：

```rust
pub(crate) struct FixtureObservationLedgerV1 { /* sealed；字段私有 */ }

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

绑定到既有冻结类型（**复用，禁止重定义**）：

- `AppendStartedRequestV1` / `AppendTerminalRequestV1` —— `model/request.rs`（`deny_unknown_fields`）；
- `ObservationReceiptV1 { request_sha256, event_sha256, sequence, root_generation, root_sha256, recorded_at_ms }`
   与 `FixtureAttemptObservationV1` —— `model.rs`；
- `ExecutionAttemptKeyV1` —— `ids.rs`；
- `ObservationStoreError` —— `error.rs`（八变体冻结）；
- structural 侧只消费 `FixtureObservationStoreV1::{open_fixture, structural_state, commit_structural}`
  与 `FixtureStructuralCommitV1` / `FixtureStoredEventV1` / `FixtureStructuralStateV1`。

## 3. 所有权、锁序与状态机

- facade 持有且仅持有：一个 `FixtureObservationStoreV1`、一个 facade 级
  `Mutex<FacadeState>`（head equality 缓存：`root_sha256/generation/event_watermark`）、
  一个可选 test-only clock seam。**锁序固定：facade state → structural store 事务**；
  锁内禁止任何外部 callback、禁止 I/O 以外的用户代码回调。
- 单 mutex 覆盖完整线性化区间：poison check → head equality（facade 缓存 ==
  `structural_state()`，不等则以 store 为准并重建缓存）→ transition/idempotency 判定 →
  clock 采样 → bundle 构造 → `commit_structural` → facade state 更新。同 handle 的并发
  append 串行化；至多一个返回 `Ok` 的路径会更新状态。
- **poison**：任何在锁内以错误路径展开（含注入故障）后进入不可恢复不确定态的情形，
  facade state 标记 poisoned；此后本 handle 的 `append_*` 与 `read_attempt` 一律
  `ObservationStoreError::Poisoned`。`CommitIndeterminate` ⇒ 立即 poison。重新打开只相信
  authoritative active（经既有 startup/typestate 路径），不读 candidate、不读孤儿。
- **candidate 状态机**：facade 永不 promote、永不写、永不读 candidate；候选槽仅由
  structural store 的既有 publish/recovery 语义管理。restart 状态机即
  `slots::startup` 既有分类（E/E、E/P、P/E、P/P 与 fresh-genesis 例外），facade 不新增分支。

## 4. 幂等与冲突（语义合同）

- canonical request 相等以 `hash::started_request_sha256` / `terminal_request_sha256`
  的输入（RFC8785-JCS + 域分隔）为准；facade 在**读取 clock、分配 sequence、构造任何
  object 之前**完成幂等判定（根因不变量 §6）。
- **幂等命中**（同 canonical request）：返回**首次** receipt（由 accepted event/root
  identity 重建：event_sha256/root_sha256/sequence/root_generation/recorded_at_ms 均取自
  已接受对象，不得用当次时钟或当次 head 重算），且磁盘零变化（root/pointer/candidate/
  inventory/clock 不变）。
- **冲突**：同 key 的不同 Started → `InvalidRequest`（typed，语义为 duplicate/conflict 的
  既有类别）；Terminal 对未 Started / 重复 Terminal / policy/runtime/outcome/evidence
  rebind → `TransitionConflict`（零 mutation）。same-Terminal 并发必须收敛到同一首次
  receipt；different-Terminal 并发只允许一个成功。
- receipt 只证明本地 `unverified_fixture` ledger commit（`ATTESTATION_STATE` 不变），不证明
  执行、外部副作用、身份、evidence 授权或 VEG。

## 5. Writer clock 与 test seam

- 唯一时钟字段是事件/root 的 `recorded_at_ms`：`max(system_now_ms, previous_accepted_recorded_at_ms)`，
  且必须 ≤ `2^53 - 1`（JSON-safe integer 上限；越界 → typed `LimitExceeded`，不静默截断）。
- 同毫秒单调靠 sequence/generation 保证；clock 回拨由 max 语义吸收；幂等命中不消费 clock。
- test-only seam：`#[cfg(test)] fn set_clock_for_test(...)` 形式的注入器（facade 私有），
  供 R4 语料驱动 rollback/同毫秒/溢出用例。生产路径禁止任何可注入时钟。

## 6. 唯一推导顺序（request → receipt）

一次 append 的对象构造顺序冻结为（全部 digest 由 `hash.rs` 域分隔函数重算，caller 提供
的任何摘要仅作校验输入、绝不直接采信）：

```
canonical request → request_sha256
                  → stored event（stamp：sequence=root_generation=watermark+1）
                  → event_sha256 → segment（previous_segment=head）→ segment_sha256
                  → current_view（reducer 唯一实现推导）→ view_sha256
                  → root（previous_root=head root）→ root_sha256
                  → FixtureStructuralCommitV1 → commit_structural（candidate→EXCHANGE→active）
                  → ObservationReceiptV1（仅由上述 accepted identity 构成）
```

startup、append planning、current-view derivation、restart 后的 receipt 重建必须共用
WP3A 已冻结的**同一个** reducer（`store/reducer.rs` 或 reader 侧既有实现二选一收敛，
禁止出现第二份 reducer）。

## 7. 边界（禁止）

- 禁止修改：`src/cas/**`、`src/bin/**`、`src/api/**`、`src/kernel/**`、`src/scheduler/**`、
  `src/mcp/**`、`src/tool/**`、`Cargo.toml`、`Cargo.lock`、public operation catalog、
  model/hash/error/ids 的既有冻结字段与枚举。
- facade 不得持有/暴露宿主路径、candidate、`put/publish` 原语或 generic ledger 能力；
  不得为测试扩大生产 API；default-off 生命周期零资源变化。
- 允许新增/修改（B 阶段）：`src/memory/execution_observation/store/facade.rs`、
  `store/facade/tests.rs`、`store/reducer.rs`（如需从 reader 收敛）、`store/clock.rs`、
  `store/mod.rs`（仅模块声明与 re-export）、`src/memory/execution_observation/mod.rs`
  （仅模块声明）、`reader/**`（仅当 reducer 收敛需要，须在 diff 中单列）。
  以 `wp3b1_spec.json` 的 machine-readable 清单为准。

## 8. 验收口径

R4 反例矩阵见 handoff §7（12 类），machine-readable 语料见
`scripts/milestones/v53/wp3b1_corpus/corpus.json`；scope 验证命令见
`scripts/milestones/v53/wp3b1_verify.py`（local-only）。任一 P0 或真值/权限/持久化 P1
即 NO-GO。本 ADR 不改变 ADR-0007/0008/0009 的任何已冻结承诺。
