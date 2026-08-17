# ADR-0008：Execution Observation Durable Store Substrate v1

- 状态：Accepted（R1→WP2 architecture contract；C2 R2 NO-GO；WP2-R2 remediation pending）
- 日期：2026-08-17
- 上位宪法：[Soul 3.1](../../system-v3.md)
- 前置合同：[ADR-0007](./0007-execution-observation-ledger-v1.md)
- 实施检查点：原 [v53 WP2 Store Checkpoint](../milestones/v53-wp2-checkpoint.md)；现行
  [WP2-R2 remediation checkpoint](../milestones/v53-wp2-r2-checkpoint.md)

## 决策

WP2 只实现一个 crate-private、默认未接线的 durable transaction substrate。它负责固定 namespace、同一
PersonalVault lease、严格且有界的物理读写、immutable object、active/candidate 双槽 publish 与 F06
pre-exchange 语义。它不实现 `FixtureObservationLedgerV1` facade、attempt current view、全链 replay、
`append_started`、`append_terminal`、`read_attempt` 或 receipt；这些必须在 R2 接纳后由 WP3 一次性从完整、
已验证的 event→segment→view→root 链派生。

这项分层修正解决 ADR-0007 最终态 API 与原 v53 WP2/WP3 实施顺序之间的歧义；它不改变 ADR-0007 的最终
wire schema、hash domain、状态机或 public boundary。

## CAS capability boundary

架构组先冻结一个固定 namespace 的 sealed CAS capability。observation 不得获得通用
`ImmutableLedgerStorage`，也不得获得路径、任意 namespace 或无界 read/list：

```text
ExecutionObservationFixtureStorage
  open(Arc<PersonalVaultStorage>)
  put_immutable_bounded(hash, bytes, maximum_bytes)
  get_immutable_bounded(hash, maximum_bytes)
  read_active_bounded(maximum_bytes)
  read_candidate_bounded(maximum_bytes)
  list_immutable_hashes_bounded(maximum_entries)
  publish_active(pointer_bytes)
```

- namespace 固定为 `ExecutionObservationFixture` / `execution-observation-fixture-ledger`；
- 只能复用调用方提供的现有 `Arc<PersonalVaultStorage>`，不得调用 `PersonalVaultStorage::open`；
- observation production source 中该 owner 只允许作为 `open_fixture` 的 `Arc<PersonalVaultStorage>` 参数出现，且必须
  恰好一次作为 `ExecutionObservationFixtureStorage::open(vault)` 的实参被消费；不得 clone、解引用、取得
  `object_cas_root`、保存 method value 或调用其任何其他 capability；
- 普通 kernel/vault lifecycle 不取得 handle，不创建目录、slot、writer、thread 或 genesis；
- 新 namespace 可以创建 exact `objects/`、`roots/active`、`roots/candidate`；既有 namespace 必须先严格验证
  目录 `0700`、文件 `0600`、无 symlink/special/missing slot，禁止 chmod、补槽或 repair 后继续；
- 同一 vault lifecycle 的第二次 claim 返回 `NamespaceAlreadyClaimed`；
- immutable collision 比较必须先按 object-kind cap 有界读取；禁止调用无界 collision read；
- production code 不扫描 newest root，不把 candidate 中的任意 bytes 当作 authoritative，也不删除 orphan。唯一
  恢复特例是 fresh `E/P(G0)`：实现从冻结常量重新构造并验证 exact genesis objects/pointer，确认 candidate
  bytes 完全相等后重新执行 publish；它不得直接采纳 candidate、选择 newest 或把该规则推广到非 genesis。

架构 base 中该 capability 已完成并保持 crate-private、default-off；允许 `dead_code` 只用于 B2 尚无调用方的
冻结 seam。WP2 candidate 不得改动 `cas/mod.rs`、高扇入 `ledger_store.rs` 或该 capability 实现。

## WP2 structural store

WP2 developer 只能增加私有 `execution_observation::store` 子模块。其职责是：

1. 按 kind 在解析前执行 bounded read；
2. 对 pointer/root/segment/stored-event 做 JCS、schema、domain hash、ordinal 与直接引用验证；
3. fresh E/E 只允许 deterministic genesis object subset；不得以全目录“最大 generation”选 head；
4. 对 active/candidate closed physical states 做验证；active 始终唯一 authoritative。除上述重新推导 exact
   genesis 的 `E/P(G0)` publish retry 外，candidate 永不自动提升；
5. 提供仅供后续 WP3 使用的 sealed structural state，不提供 attempt lookup、receipt 或 raw setter；
6. publish 前错误保持旧 active；exchange 后 sync 不确定映射 `CommitIndeterminate` 并 poison 当前 handle。

WP2 冻结的唯一 observation-side seam 为：

```rust
pub(super) enum FixtureStoredEventV1 {
    Started(StoredStartedEventV1),
    Terminal(StoredTerminalEventV1),
}

pub(super) struct FixtureStructuralCommitV1 {
    pub(super) event: FixtureStoredEventV1,
    pub(super) segment: FixtureEventSegmentV1,
    pub(super) current_view: FixtureCurrentViewV1,
    pub(super) root: FixtureLedgerRootV1,
}

pub(super) struct FixtureStructuralStateV1 {
    pub(super) root_sha256: String,
    pub(super) generation: u64,
    pub(super) event_watermark: u64,
}

pub(super) struct FixtureObservationStoreV1 { /* sealed */ }

impl FixtureObservationStoreV1 {
    pub(super) fn open_fixture(
        vault: Arc<PersonalVaultStorage>,
    ) -> Result<Self, ObservationStoreError>;

    pub(super) fn structural_state(
        &self,
    ) -> Result<FixtureStructuralStateV1, ObservationStoreError>;

    pub(super) fn commit_structural(
        &self,
        commit: FixtureStructuralCommitV1,
    ) -> Result<FixtureStructuralStateV1, ObservationStoreError>;

    #[cfg(test)]
    pub(super) fn inject_pre_exchange_failure_once(&self);

    #[cfg(test)]
    pub(super) fn inject_post_exchange_sync_failure_once(&self);
}
```

`commit_structural` 接受 typed、完整的 event/segment/view/root bundle，自行重算全部 domain hash 和 pointer；不
接受 caller pointer、path、receipt、raw root setter 或 hash override。返回值只含已发布 active head 的结构身份，
不是 action/attempt receipt。它必须验证新 root 是当前 active 的唯一直接 child；WP2 只校验 bundle 的结构绑定，
不从事件推导 attempt state。WP3 必须在调用前完成全量 replay，并在下一 checkpoint 将 facade 与 receipt 一次性
冻结。

WP2 可以沿最多 20,000 个已引用 event 读取；允许未引用 orphan，因此不得把“全目录对象总数”当 ledger
容量或拒绝合法 active 链。fresh E/E 的对象检查必须常量内存、只接受可重算 genesis 子集。

## 新增有界值与错误映射

- `STORED_EVENT_MAX_BYTES = CANONICAL_REQUEST_MAX_BYTES + 4096 = 135168`；
- pointer 4096、root/segment 65536、current view 8388608 的既有上限不变；
- raw bytes 超限必须在 JSON deserialize/JCS 前拒绝；
- 读取持久化对象后出现的 caller-input、transition 或 limit 类错误不得原样透出，统一映射为
  `CorruptStore`；资源/byte/count 超限使用稳定 `stored_resource_limit`；
- I/O 只映射 `StorageUnavailable`，publish 已 exchange 但 durability 未确认映射 `CommitIndeterminate`，随后
  所有读写返回 `Poisoned`；错误和 tracing 只含稳定 category，不含 path、正文、完整 CID/hash 或底层消息。

## 明确延后到 WP3

- 全量 current-view rebuild 与 attempt transition；
- `FixtureObservationLedgerV1` exact API；
- `ObservationReceiptV1` 与 `FixtureAttemptObservationV1` 的派生和验证；
- Open/Terminal restart equality、duplicate Started/Terminal、并发 terminal；
- 任何 live producer、credential/evidence authorization 或 kernel/scheduler wiring。

WP2 通过只能声明 durable substrate 的物理与结构不变量成立，不能声明 execution observation 已成为产品能力。

## 验收

R2 只在以下全部满足时 GO：WP1 corpus 累计通过；F06 candidate-before-exchange 不改变 active 且不自动
promote；strict topology 不自修复；所有读/碰撞有界；stored 错误映射稳定；共享 CAS anchor 精确；default-off
lifecycle 无新增 mutation；开发 diff 不包含 current-view/replay/recovery/live wiring。任一项失败即 NO-GO。
