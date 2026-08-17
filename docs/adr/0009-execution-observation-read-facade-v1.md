# ADR-0009：Execution Observation Read Facade v1

- 状态：Accepted（仅 WP3A internal fixture）
- 日期：2026-08-18
- 前置：ADR-0007、ADR-0008 与 R2 acceptance

## 决策

WP3A 只实现基于 authoritative active chain 的确定性只读重建。它提供 crate-private fixture facade，将已经由
ADR-0008 structural store 全量验证的事件按唯一 reducer 重建为 attempt state；不增加 durable writer、append、clock、
public API、kernel/scheduler wiring 或自动学习。

```rust
pub(crate) struct FixtureObservationReaderV1 { /* sealed */ }

impl FixtureObservationReaderV1 {
    pub(crate) fn open_fixture(
        vault: Arc<PersonalVaultStorage>,
    ) -> Result<Self, ObservationStoreError>;

    pub(crate) fn read_attempt(
        &self,
        key: &ExecutionAttemptKeyV1,
    ) -> Result<Option<FixtureAttemptObservationV1>, ObservationStoreError>;
}
```

startup replay、read_attempt 与 current-view 校验必须调用同一个 pure reducer。reducer 输入是按 sequence 升序排列、
已经过 schema/hash/ordinal/chain 验证的 stored event；输出按 canonical attempt key 排序。任何 duplicate Started、Terminal
without Started、cross-attempt rebind、第二个不同 Terminal、stored view 与 reducer 结果不等，均为稳定 `CorruptStore`。

## 边界

- 只读 facade 只能消费 sealed store；不得获得 CAS path、raw writer、candidate slot 或 generic ledger。
- 不相信 stored current view；必须独立 replay 后逐字段、逐 hash 比较。
- receipt、append 幂等、writer time、transaction mutation 属 WP3B，未授权。
- 不证明真实执行、外部副作用、identity、evidence 可读性/权限或 VEG。
- 无 public operation/schema 变化；`plico.personal.v2` 保持不变。

## 验收

架构语料至少覆盖：空链、Open、Terminal、同 execution 多 attempt、乱序输入拒绝、duplicate Started、Terminal without
Started、不同 Terminal、stored view tamper、重启重建一致、20,000 event 上限和 default-off 零产品 wiring。任一 reducer
复制、raw CAS 旁路或 append 能力出现即 NO-GO。
