# ADR-0007：Execution Observation Ledger v1

- 状态：Accepted（Architecture contract；implementation not started）
- 日期：2026-08-16
- 宪法依据：[Soul 3.1](../../system-v3.md)
- 产品边界：[ADR-0001：个人数字分身与记忆原生数据模型](./0001-personal-digital-twin.md)
- 公共契约：[ADR-0003：个人数字分身统一领域模型与公共能力契约](./0003-personal-twin-domain-and-public-capability-contract.md)
- Truth Firewall：[ADR-0004：Canonical Revision Ledger](./0004-canonical-revision-ledger.md)
- 研究输入：[ADR-0006：Verified Experience Runtime](./0006-verified-experience-runtime.md)
- 实施合同：[v53 Execution Observation Ledger Core](../milestones/v53-execution-observation-spine.md)

## 状态与规范效力

本 ADR 接受的只是 Phase 1A ledger core 架构合同，不是实现完成声明。当前代码不得被描述为已经拥有
execution-observation writer、可信 producer、evidence authorization、真实执行 coverage 或公共能力。

ADR-0006 仍为 Proposed；本文不会把其 Phase 1B、learning、promotion 或 branch runtime 提升为 Accepted。
实现只能在 v53 的 R0 handoff 冻结本文 digest、implementation-base SHA、schema golden vectors、工具链和
scope gate 后开始。

R0 handoff 使用 `plico.milestone.v53/2`：packet 不携带用户名、Home、checkout 或工具绝对路径；正式
authorization/scope 仅由架构组受控 runner 执行，第三方只提交 Git candidate。该交接修订不改变本文的
ledger wire schema、namespace 或产品语义。

## 1. 决策范围

本 ADR 只授权一个 crate-private、未接生产运行时的 Phase 1A fixture ledger：

- 保存严格格式的 CID 字符串引用，不保存正文；
- 所有 record 固定为 `unverified_fixture`；
- 支持 Started/Open/Terminal、幂等、冲突、重启验证和故障关闭；
- 不接入 kernel、scheduler、config、API、MCP、CLI 或 daemon；
- 不验证 CID 存在性、完整性、可读性或权限；
- 不产生 canonical memory、projection、claim、procedure、skill、task 或工具行为。

本 ADR 不增加 `plico.personal.v2` capability。Fixture ledger 的测试可以证明存储状态机和完整性，不能证明
其中字段与现实执行、可信身份或已授权 evidence 一致。

## 2. Namespace 与存储拓扑

固定新增：

```rust
ImmutableLedgerNamespace::ExecutionObservationFixture
```

固定目录：

```text
<vault>/execution-observation-fixture-ledger/
├── objects/
└── roots/
    ├── active
    └── candidate
```

约束：

- 只能由现有 `Arc<PersonalVaultStorage>` 发放 handle；
- 不得再次打开或锁定 personal vault；
- 不得接受路径、目录名或任意 namespace 字符串；
- CAS 只增加固定 enum/opener，不导入 observation schema；
- CAS 只额外暴露固定槽读取：
  `read_candidate_bounded(maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>>`；它与
  `read_active_bounded` 同语义，不暴露路径或 writer；
- production `AIKernel` 不取得该 handle，不创建 genesis；
- 只有 observation ledger 测试显式调用 `open_fixture`；
- orphan immutable objects 可以保留，但不具有 active 语义，也不自动清理；
- 不提供 delete、compact、retention、migration 或 repair writer。

Phase 1B trusted record 不得写入这个 namespace。它必须另立 Accepted ADR、使用新 schema 和新固定
namespace；禁止把 fixture event 原地提升为 trusted event。

## 3. 固定状态模型

```text
Absent
  └─ append Started ──> Open

Open
  ├─ same Started request ──> 返回首次 receipt
  ├─ different Started request ──> Conflict
  ├─ append Terminal ──> Terminal
  └─ restart ──> Open

Terminal
  ├─ same Terminal request ──> 返回首次 receipt
  └─ different Terminal request ──> Conflict
```

- 一个 accepted attempt 恰好一个 Started，最多一个 Terminal；
- Terminal without Started 是 typed transition conflict；
- Open 重启后保持 Open；
- store 不得自动生成任何业务 terminal；
- `ObservationStoreError::CommitIndeterminate` 绝不能转换为
  `TerminalOutcomeV1::Indeterminate`。

## 4. 固定输入 schema

所有类型使用 `serde(deny_unknown_fields)`；所有 enum 使用显式 `type` discriminator 和 snake_case wire value。
所有声明字段都必须出现在 canonical wire object 中，禁止通过 `skip_serializing_if` 省略字段。以下字段是唯一
nullable 字段，值缺失必须拒绝，`None` 必须编码为 JSON `null`：

- `AppendStartedRequestV1.fixture_role_ref`；
- `AppendStartedRequestV1.fixture_session_ref`；
- `AppendTerminalRequestV1.execution_elapsed_ms`；
- `FixtureEventSegmentV1.previous_segment_sha256`；
- `FixtureAttemptViewV1.terminal_request_sha256`；
- `FixtureAttemptViewV1.terminal_event_sha256`；
- `FixtureLedgerRootV1.previous_root_sha256`；
- `FixtureLedgerRootV1.event_segment_head_sha256`；
- `FixtureAttemptObservationV1.terminal_receipt`。

实现不得依赖 `Option<T>` 对 missing field 的默认接受行为；从 JSON 读取 request 或 stored object 时必须先证明
nullable 字段存在，再解析其 `null | value`。其他字段既不可缺失也不可为 `null`。

所有 UUID wire value 固定为 36 字节小写、带连字符的 RFC 4122 文本
`xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`。`execution_id`、三个 `FixtureOriginV1` ID，以及存在时的
`fixture_role_ref`/`fixture_session_ref` 都必须 non-nil；大写、无连字符、URN、braced 或 nil UUID 拒绝。
UUID version/variant 不承载可信身份语义。

Enum wire representation 固定为 internally tagged object：origin 分别为
`{"type":"public_request","request_id":...}`、`{"type":"intent_dispatch","intent_id":...}`、
`{"type":"internal_task","task_id":...}`；unit terminal outcome 为只含 `type` 的 object，Failure 为
`{"type":"failure","category":...}`。`event_kind` 是 `"started" | "terminal"` 字符串，不是第二种
enum object 表示。

```text
ExecutionAttemptKeyV1
  execution_id: canonical non-nil UUID
  attempt: NonZeroU32
```

```text
FixtureOriginV1 =
  public_request { request_id: UUID }
  | intent_dispatch { intent_id: UUID }
  | internal_task { task_id: UUID }
```

```text
AppendStartedRequestV1
  schema = "plico.execution-observation.fixture-start-request/v1"
  key: ExecutionAttemptKeyV1
  fixture_origin: FixtureOriginV1
  attestation_state = "unverified_fixture"
  fixture_role_ref: UUID?
  fixture_session_ref: UUID?
  operation_contract_sha256: lowercase SHA-256
  input_evidence_cids: ordered CID list
  context_evidence_cids: ordered CID list
  policy_sha256: lowercase SHA-256
  runtime_sha256: lowercase SHA-256
```

```text
AppendTerminalRequestV1
  schema = "plico.execution-observation.fixture-terminal-request/v1"
  key: ExecutionAttemptKeyV1
  attestation_state = "unverified_fixture"
  outcome: TerminalOutcomeV1
  output_evidence_cids: ordered CID list
  execution_elapsed_ms: JSON-safe u64?
  policy_sha256: lowercase SHA-256
  runtime_sha256: lowercase SHA-256
```

`TerminalOutcomeV1` 的闭集为：

```text
success
failure { category: FailureCategoryV1 }
timeout
cancelled
indeterminate
```

`FailureCategoryV1` 的闭集为：

```text
invalid_input
policy_denied
dependency_unavailable
executor_rejected
executor_failed
executor_panicked
tool_failed
internal
```

禁止 `unknown`、自由文本 message 或第二个 `error_category` 字段。Terminal 的 `policy_sha256`、
`runtime_sha256` 必须与 Started 完全相等；否则返回 rebind conflict，旧 root 不变。

## 5. Collection 与数值限制

- CID 与 digest 必须是精确 64 字节小写 ASCII hex；
- 三个 CID 字段都是 ordered list，顺序进入 request hash；
- 单个 list 内 duplicate 拒绝；不同 list 间允许相同 CID；
- 每个 CID list 最多 256 项，三个 list 合计最多 512 项；
- attempt 范围是 `1..=u32::MAX`；
- 所有 JSON 整数必须位于 `0..=2^53-1`；
- `execution_elapsed_ms` 只是 fixture assertion，不证明真实执行耗时；
- 最多 10,000 个 attempt、20,000 个 stored event；
- 达到 attempt 上限后拒绝新的 Started，但允许已有 Open attempt写 Terminal；
- canonical request 最大 128 KiB；
- pointer 最大 4 KiB，root/segment 最大 64 KiB，current view 最大 8 MiB；
- 超限必须在写任何 immutable object 前失败。

## 6. Stored event schema

Caller request 与 writer-stamped event 严格分层：

```text
StoredStartedEventV1
  schema = "plico.execution-observation.fixture-started/v1"
  request: AppendStartedRequestV1
  request_sha256
  sequence
  root_generation
  recorded_at_ms
```

```text
StoredTerminalEventV1
  schema = "plico.execution-observation.fixture-terminal/v1"
  request: AppendTerminalRequestV1
  request_sha256
  sequence
  root_generation
  recorded_at_ms
```

规则：

- request hash 只覆盖 caller-controlled canonical request；
- retry identity 不包含 sequence、generation 或 recorded time；
- sequence、generation、recorded time 只能由 writer 产生；
- `recorded_at_ms` 是 ledger record time，不是 execution start/finish time；
- writer time 固定为 `max(system_now_ms, previous_recorded_at_ms)`；
- 相同 request retry 必须返回首次保存的 stamp/receipt。

## 7. Segment、Current View、Root 与 Pointer

每个新 event 使用一个 immutable segment，不支持 batch：

```text
FixtureEventSegmentV1
  schema = "plico.execution-observation.fixture-segment/v1"
  first_sequence
  last_sequence                 # 必须等于 first_sequence
  previous_segment_sha256?
  event_kind                    # started | terminal
  event_sha256
```

```text
FixtureAttemptViewV1
  key
  attestation_state = "unverified_fixture"
  started_request_sha256
  started_event_sha256
  terminal_request_sha256?
  terminal_event_sha256?
```

```text
FixtureCurrentViewV1
  schema = "plico.execution-observation.fixture-current-view/v1"
  attestation_state = "unverified_fixture"
  generation
  event_watermark
  attempts[]                    # execution UUID bytes、attempt 升序
```

```text
FixtureLedgerRootV1
  schema = "plico.execution-observation.fixture-root/v1"
  trust_class = "unverified_fixture_only"
  generation
  previous_root_sha256?
  event_segment_head_sha256?
  event_watermark
  current_view_sha256
  committed_at_ms
```

```text
FixtureActivePointerV1
  schema = "plico.execution-observation.fixture-root-pointer/v1"
  root_sha256
```

Genesis 固定为：generation/event watermark 为 0；segment head 与 previous root 为 `None`；current view
为空；committed time 为 0。任一 pointer slot 缺失、不是 owner-only regular file 或超限都 fail closed。Genesis pointer publish
不确定时返回 `CommitIndeterminate`，不返回 store handle。

### 7.1 Dual-slot startup state machine

定义 `E` 为零字节 pointer slot，`P(Rn)` 为 canonical JCS pointer，且其完整 root chain 验证后指向
generation `n` 的合法 root。`active` 是唯一 authoritative slot；`candidate` 只说明最近一次物理 publish
可能处于哪个阶段，绝不自动获得 authority。

| Active | Candidate | 含义 | 启动行为 |
|---|---|---|---|
| `E` | `E` | fresh 或 genesis pointer 写入前失败 | objects 为空时创建 genesis；objects 只能是可重算 genesis view/root 的合法子集，否则 fail closed |
| `E` | `P(G0)` | genesis candidate 已 durable、尚未 exchange | 完整验证 exact genesis；用同一 pointer 重新执行正常 publish，成功后成为 `P(G0)/E` |
| `P(G0)` | `E` | genesis 正常完成 | 接受 active |
| `P(Rn)` | `P(Rn-1)` | 正常 publish 后 candidate 保存旧 active | 接受 active；active 的 previous root 必须精确等于 candidate root |
| `P(Rn)` | `P(Rn+1)` | candidate 已 durable、exchange 前失败 | 接受 active；完整验证 candidate 是 active 的唯一直接 child，但绝不自动 promote |
| 其他 | 其他 | 损坏、不可能状态或不完整回拨 | `CorruptStore`，禁止继续写 |

直接 parent/child 必须同时满足：generation 与 event watermark 都相差精确 1；child previous root 与 previous
segment head 精确绑定 parent；child 恰好增加一个合法 event；child current view 可由 parent view 加该 event
重建；两个 pointer 及其 root/segment/event/view 都是 canonical JCS 且 hash 正确。以下状态明确拒绝：两个槽
指向同一 root；active generation 大于 0 而 candidate 为空；两个合法 root 不是直接 parent/child；candidate
非 canonical、缺 object 或 view 不匹配；active 为空而 candidate 不是 exact genesis；fresh inventory 出现
deterministic genesis 集合之外的 object。

启动时必须验证：

- active pointer 是 canonical JCS；
- candidate slot 符合上述 closed state machine；
- pointer→root→previous roots 完整；
- segment sequence 连续、previous hash 完整；
- 每个 event hash/schema/request hash 正确；
- state transition 合法；
- current view 与全量事件重建结果字节一致；
-所有 generation/watermark 单调且一致；
- unknown/future schema、tamper、重复 Started/Terminal 全部 fail closed。

启动不扫描并选择 objects 中 generation 最大的 root，也不因 candidate 或 orphan 中存在更高 generation 而
自动提升。未被 active 采用的 immutable object 不构成 owner 接受证据。

## 8. JCS 与 hash domain

固定使用仓库已有的 `serde_json_canonicalizer = 0.3.2` 与 SHA-256。禁止新增 canonicalization 依赖、
普通 JSON serialization、手写 key sort 或 normalization wrapper。

固定 domain：

```text
plico.execution-observation.fixture.started-request.v1\0
plico.execution-observation.fixture.terminal-request.v1\0
plico.execution-observation.fixture.started-event.v1\0
plico.execution-observation.fixture.terminal-event.v1\0
plico.execution-observation.fixture.segment.v1\0
plico.execution-observation.fixture.current-view.v1\0
plico.execution-observation.fixture.root.v1\0
```

计算方式：

```text
sha256(domain || RFC8785_JCS(value))
```

Pointer 不另设 identity hash；其 canonical bytes 只包含 root hash。读取任何对象时必须重新 canonicalize、
比较原始 bytes，并验证 domain hash 与文件名一致。

R0 golden vectors 必须冻结一个 exact genesis → Started → Terminal chain：三个 request optional 字段用显式
`null`，Open view 的两个 terminal hash 和 genesis root 的两个前驱字段也用显式 `null`；覆盖七个 hash domain、
三个 canonical pointer，以及 request→event→segment→view→root 的每条 hash binding。只提供 request vectors
或只重算各对象自身 digest 不构成兼容性证明；verifier 必须同时固定 vector digest 并验证跨对象引用。

禁止复用或导入 `memory/ledger/hash.rs`；允许直接依赖同一个外部 JCS crate。

## 9. Commit 与故障语义

非幂等 append 固定流程：

```text
validate request
→ lock writer
→ validate transition/current binding
→ allocate sequence/generation/time
→ write event object
→ write segment object
→ write rebuilt current-view object
→ write root object
→ publish active pointer
→ update in-memory verified state
→ return receipt
```

- pointer publish 前失败：返回稳定 storage error；旧 root 仍 active；
- candidate 已 durable、exchange 前失败：active 旧、candidate 是其直接 child；返回稳定 storage error，child
  不算 accepted，后续 publish 可以覆盖 candidate；
- pointer exchange 后 parent fsync 失败：返回 `ObservationStoreError::CommitIndeterminate`，writer 进入
  `Poisoned`；当前进程禁止任何后续读写结论；
- 必须 drop/reopen 后按 active pointer 全量验证；
- 不确定提交不得自动 retry，也不得写成 fixture terminal；
- immutable orphan 不代表 accepted；
- publish 成功后才更新内存 current view 并返回 receipt。

Poison 后 `append_started`、`append_terminal` 与 `read_attempt` 都只返回 `Poisoned`；不得根据内存 cache 返回
观察结论，不自动 retry 或改写 candidate。drop/reopen 后可能看到 `active=new/candidate=old`，也可能看到
`active=old/candidate=prepared-child`；两者都按 7.1 全量验证。reopen 不持久化 Poisoned，也不继续返回旧的
`CommitIndeterminate`；调用方只能按原 key 读取 authoritative active view 来 reconcile。

## 10. Exact crate-private API

唯一允许的 API：

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

Receipt：

```text
ObservationReceiptV1
  request_sha256
  event_sha256
  sequence
  root_generation
  root_sha256
  recorded_at_ms
```

Read result 必须始终携带 `attestation_state=unverified_fixture`。

`FixtureAttemptObservationV1` 的 exact shape 固定为：

```text
FixtureAttemptObservationV1
  key: ExecutionAttemptKeyV1
  attestation_state = "unverified_fixture"
  started_receipt: ObservationReceiptV1
  terminal_receipt: ObservationReceiptV1 | null   # 字段必须存在；Open 时为 null
```

`read_attempt` 对 Absent 返回 `None`；对 Open 返回上述 object 且 `terminal_receipt=null`；对 Terminal 返回首次
Started/Terminal 的两个 receipt。receipt 必须从已验证事件/root chain 重建，不能使用未验证 cache 或重新分配
time/sequence/generation。

不允许：

- `list/history/export/delete/compact/repair`；
- raw writer、raw root setter、arbitrary read-by-path；
- public re-export；
- trait object producer sink、callback 或 hook；
- background thread、worker、queue 或 global singleton；
- 从环境变量或配置自动打开。

测试专用 clock/fault injector 必须是 `#[cfg(test)]`，且不能进入 production constructor。

## 11. Exact error contract

```text
ObservationStoreError =
  InvalidRequest { category: InvalidRequestCategory }
  | TransitionConflict { category: TransitionConflictCategory }
  | LimitExceeded { category: LimitCategory }
  | CorruptStore { category: CorruptionCategory }
  | StorageUnavailable
  | NamespaceAlreadyClaimed
  | CommitIndeterminate
  | Poisoned
```

`InvalidRequestCategory` 至少冻结为：

```text
unsupported_schema
invalid_attestation
nil_uuid
zero_attempt
invalid_digest
invalid_cid
duplicate_cid
invalid_failure_category
unsafe_integer
size_limit_exceeded
jcs_canonicalization_failed
```

`TransitionConflictCategory` 冻结为：

```text
started_already_bound
terminal_without_started
terminal_already_bound
terminal_policy_rebind
terminal_runtime_rebind
```

`LimitCategory` 冻结为：

```text
attempt_limit
event_limit
evidence_list_limit
evidence_total_limit
request_bytes_limit
object_bytes_limit
```

`CorruptionCategory` 冻结为：

```text
missing_active_pointer
noncanonical_pointer
unsupported_stored_schema
object_hash_mismatch
broken_root_chain
broken_segment_chain
sequence_gap
generation_mismatch
duplicate_started
duplicate_terminal
invalid_transition
current_view_mismatch
invalid_candidate_state
```

Error display、trace 和 evidence 只允许稳定 category；不得包含正文、完整 CID/hash、role ref、宿主路径或
底层 provider/storage message。底层 `std::io::Error` 即使作为 private source 保存，也不得直接格式化或 tracing。

## 12. 依赖 allowlist

`src/memory/execution_observation/**` 只能依赖：

- `crate::cas::{PersonalVaultStorage, ImmutableLedgerNamespace, ImmutableLedgerStorage, LedgerStorageError}`；
- `serde`、`serde_json`；
- `serde_json_canonicalizer`；
- `sha2`；
- `uuid`；
- `thiserror`；
- `std`。

`crate::util` 仅允许复用无 I/O 的时间/安全数值 helper；若语义不同，应在模块内实现 private helper，不能
建立兼容层。CAS 对 observation 只知道固定 namespace，不得导入 observation model/hash/error。

## 13. 禁止依赖

Observation 模块不得导入或调用：

- Memory ledger/model/hash/current view；
- `LayeredMemory`、MemoryEntry、MemoryId/RevisionId；
- projection manifest/runtime；
- kernel、scheduler、intent、tool；
- API/public protocol、client、MCP、CLI；
- SemanticFS、object search、KG、embedding、LLM；
- EventBus、trace、cost/session/task JSONL；
- permission store、AgentKeyStore；
- cognition、TrajectoryTracker、ExperienceMiner、SkillForge；
- config/env provider；
- benchmark evaluator。

目录位于 `memory/` 不赋予其使用 Memory canonical 类型的权限。

## 14. Threat model 与诚实边界

本 ledger 提供：

- domain-separated content integrity；
- active root 指向的内部 root/segment/event 链完整性；
- current view 可重建性；
- 单进程 writer 序列化和同一 vault lifecycle 的互斥。

本 ledger **不提供 authenticated anti-rollback**。拥有 personal vault 写权限的 same-UID 进程、被恢复的
旧备份或可替换 `roots/active`/`roots/candidate` 的主体，可以把这两个槽和对象集合一起回拨到密码学合法的
历史 `Rn/Rn-1`，或构造合法的 `Rn/Rn+1` prepared-child 组合。该 pair 的 JCS、hash、previous-root chain 和 current view 都可能完全有效，因此 startup full validation
只能证明“这是一个内部一致的历史前缀”，不能证明“这是 owner 最近接受的 head”。

Phase 1A 不得把 root generation、wall-clock time、candidate slot、目录 mtime、事件数量或当前进程内缓存
描述成 authenticated freshness。它也不使用网络时间、TPM、远端 witness、单调硬件计数器或 owner-signed
head。发现 pointer 指向损坏或不自洽的对象时必须 fail closed；遇到密码学合法旧 root 时，本版本没有可靠
办法区分 rollback、诚实的 pre-exchange orphan 与 owner 有意恢复，只能诚实报告已验证的 active root
identity/generation，不能宣称
anti-rollback verified。

如果 Phase 1B 或后续产品需要 rollback detection，必须另立 Accepted ADR，冻结 owner-signed monotonic head、
可信硬件计数器或外部 witness 中至少一种真实性来源，以及备份恢复、密钥轮换、离线设备和灾难恢复语义。
不得通过在本 ledger 内再写一个可由 same-UID 同时回拨的“last seen generation”文件伪装解决。

其他非目标威胁包括：已攻陷进程可以在写入前伪造 fixture 字段；Phase 1A 不验证 CID 指向内容；fixture
outcome 不证明工具副作用是否发生；host root 或物理攻击者不在本地文件权限边界内。以上限制不降低对
corruption、partial publish、hash mismatch 和非 canonical bytes 的 fail-closed 要求。

## 15. Phase 1B 硬阻断

以下任一能力都必须新立 Accepted ADR 和独立里程碑：

1. kernel/scheduler/live tool producer；
2. trusted execution context 或 credential-bound role；
3. CID existence、integrity、readability、authorization verifier；
4. begin-before-action 与 side-effect ordering；
5. timeout/cancel 的真实语义；
6. Open attempt 自动收敛或 recovery terminal；
7. production config、feature、lazy runtime initialization；
8. trusted observation schema/namespace；
9. fixture→trusted migration 或 promotion；
10. public reader/list/history/export；
11. feedback、procedure、claim、skill、ranking 或 action；
12. retention、hard erase、compact 或 repair；
13. authenticated anti-rollback 或 owner-accepted head freshness。

Phase 1B 必须使用新 trusted schema 和新 fixed namespace；不得复制、重写或提升 fixture record。若研究需要
关联，只能由 trusted record 单向引用 fixture event hash，并明确其未验证来源。

## 16. 接纳后仍然不能声明

即使本 ADR 实现完成，也只能声明：

> internal, unconnected, unverified-fixture execution-observation ledger core

不能声明：

- credential-bound；
- evidence verified；
- real execution coverage；
- exactly-one terminal for live attempts；
- runtime recovery；
- authenticated anti-rollback；
- public capability；
- Verified Experience Gain product gate。

## 17. 验收与停止条件

实现验收必须遵循 v53 的 WP0→WP6 与 R0→R6。至少证明：

- namespace 只由同一 `PersonalVaultStorage` lease 发放；
- production kernel/daemon lifecycle 零 observation-added mutation；
- request/stored event、JCS/domain、ordered list、limits 和 golden vectors 完整；
-相同 request 幂等，不同 Started/Terminal 与 policy/runtime rebind 均 conflict；
- Open/Terminal 重启结果与字节一致；
- pre-publish failure、post-exchange `CommitIndeterminate`、Poisoned 和 reopen 语义分离；
- tampered pointer/root/segment/event/view fail closed；
- store uncertainty 不生成 terminal；
- logs/error/evidence bundle 无正文、凭据、私有路径和完整 CID/hash；
- public exact-14、Memory ledger、projection、KG、skill、profile、retrieval 与 benchmark evaluator 不变；
- scope gate 对禁止依赖、denylist path、macro/feature/workflow 侧门非零退出。

任一实现需要 live producer、真实 identity/evidence verdict、Memory ledger schema 变更、第二个 vault lock、任意
path namespace、兼容层、dual write 或 production auto-open 时，Phase 1A 立即停止并回到架构评审。

## 18. 后果

正面后果：Phase 1B 将获得一个已验证的 append-only 状态机和故障语义，而不会提前污染 canonical memory、
权限或公共产品面；fixture 与 future trusted truth 在 schema 和 namespace 上物理分离。

代价：Phase 1A 代码没有生产消费者，fixture 不能直接升级为 trusted history；current view 使用有界完整重建，
容量和写放大只适合作为研究 core；合法旧 root rollback 仍不可认证检测。以上代价是本阶段刻意保留的安全
边界，不得通过顺手接线、扩大 limits 或伪造 freshness 消除。
