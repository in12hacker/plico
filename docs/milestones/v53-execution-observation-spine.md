# v53 里程碑：Execution Observation Ledger Core（Phase 1A）

**日期**：2026-08-16
**合同版本**：`plico.milestone.v53/1`
**状态**：Draft / Architecture Review
**产品基线**：`fe4c08260fc3e6dc0e3d37921b863a7ed48a330a`
**Architecture-Frozen identity**：由架构签名的外部 R0 handoff packet 绑定；Draft 阶段为 `PENDING`，
不在本合同内写自身 commit SHA，以避免 self-reference
**实施方**：第三方开发组
**接纳方**：Plico 架构组 + 独立 QA/安全专家
**目标**：交付一个内部、未接生产运行时、只保存 evidence CID 引用、可重启验证且失败关闭的 execution-observation 账本核心。
**范围**：Phase 1A 只做类型、验证、CAS-owned durable store、current view 和故障语义；不接 kernel/scheduler producer，不做授权 evidence adapter、学习、公共 API 或产品行为变更。

---

## 0. 合同地位与开工权限

规范优先级固定为：Soul 3.1 → Accepted ADR → `AGENTS.md` → 模块 `INDEX.md` → 本里程碑。
本文不能授权违反上位规范的实现，也不能把 Proposed ADR 提升为 Accepted。

状态机固定为：

```text
Draft / Architecture Review
  → Architecture-Frozen
  → Implementation-Candidate
  → Evidence-Complete
  → Accepted | Rejected
```

- 第三方开发组只能用外部 handoff evidence 申请 `Implementation-Candidate`，不得修改本文、summary 或 INDEX 状态。
- 只有架构组可以冻结字段、hash domain、storage namespace、可信身份来源和 allowlist。
- 只有独立 QA/安全复核完成后，架构组可以填写 `v53-summary.md` 并更新索引状态。
- v53 通过不自动把 ADR-0006 改为 Accepted，不增加 public capability。
- 进入 `Architecture-Frozen` 后，本合同正文不可原地修改；冻结身份/状态先写入架构签名的外部 R0 handoff，
  candidate/evidence/final verdict 再由验收组写入 `v53-summary.md`。任何语义修改必须发布新合同版本
  （例如 `plico.milestone.v53/2`）并重新冻结 digest/implementation base。

### R0 开工硬前置

ADR-0006 当前为 Proposed，不能单独授权新 durable writer 或 vault namespace。任何生产代码修改前，
架构组必须先审议并 Accepted 一份窄化的《Execution Observation Ledger v1》ADR，至少冻结：

1. observation 与 canonical memory、projection、telemetry 的数据归属；
2. 固定 namespace、schema、hash domain、root/pointer 与 single-writer 语义；
3. future trusted producer/evidence verifier 的输入边界，以及 Phase 1A 不作身份/授权完成声明；
4. begin/terminal、幂等、冲突、`TerminalOutcomeV1::Indeterminate`、
   `ObservationStoreError::CommitIndeterminate`、poison 与 restart recovery；
5. Phase 1A production-zero-wiring、测试显式开启与零 mutation 证明；任何后续开关都必须 lazy init；
6. 物理删除、retention、public reader、live producer 和 open-attempt recovery policy 均为 unsupported。

R0 未签字时，开发组只能提交无代码的 schema/威胁模型 review，不能创建 namespace、writer 或兼容壳。
R0 handoff packet 必须同时给出：包含冻结合同和 Accepted ADR 的 implementation-base SHA、contract/ADR digest、
逐字段 provenance/collection-semantics matrix、crate-private `open/append/read/rebuild` exact 签名与文件清单、
架构组拥有的 scope/evidence 工具、固定工具链版本，以及 fresh/existing lifecycle differential recipe。该 recipe
必须冻结操作序列、deterministic fixture/normalization、base 允许 mutation 集、observation absence 判定、
比较器命令与退出语义。

### 已知基线缺口

`.github/workflows/ci.yml` 当前仍尝试构建已删除的 `plico-sse`，且 CI 不运行 benchmark Python
pytest/ruff。架构组必须在 Architecture-Frozen 前单独修复或签发书面 waiver；在此之前“GitHub CI green”
不能代替本文的完整命令门禁。若签 waiver，R0 handoff 还必须冻结 `v53-integration` required-check 的具体
执行方式、owner 和失效日期。第三方不得顺手修改 workflow 来扩大本里程碑范围。

---

## 1. 为什么只做 Phase 1A

当前仓库已有多类“看起来像执行历史”的结构，但都不能升级为 evidence truth：

| 现有结构 | 不能作为 observation truth 的原因 |
|---|---|
| `scheduler::ExecutionResult` | 只有 bool + String；kernel JSON error 也可能被包装为 `Ok`；不能区分 typed failure |
| scheduler timeout | `spawn_blocking` 超时后任务可能继续产生副作用；不能声称 cancelled 或未执行 |
| `TrajectoryTracker` | 易失、有界、字段经常为空；没有 trusted role、attempt、policy 或 durable terminal |
| EventBus | broadcast/JSONL 是诊断与通知；append 失败不形成 durable acknowledgement |
| tool trace | 7 天 retention、异步 best-effort、直接保存 input/output，不是 Truth Firewall |
| feedback/profile/cost ledger | mutable ranking 或 session aggregate，不能证明一次 action 的 evidence 与 terminal |
| Memory ledger v1 | canonical personal memory 真值；塞入 observation 会污染 current view 与 hash domain |

因此 v53 不把任何旧 telemetry 重命名为 evidence，不解析字符串推断 terminal，也不接入 kernel/scheduler。
它先把未来真实接线依赖的最小 typed/durable ledger core 做实。真实执行边界、begin-before-action 顺序、
credential-bound adapter、evidence access verifier 和 open-attempt recovery policy 必须由 Phase 1B 独立冻结。

---

## 2. 永久不变量

1. **Truth Firewall**：真实 producer 未来必须先按授权策略把原始 input/context/output 写入 CAS；Phase 1A 只保存
   canonical-form CID 字符串引用，不保存正文、prompt、response、tool params、bearer 或动态 error。它不验证
   引用已入 CAS；unresolved CID 仍只是 `unverified_fixture`。
2. **独立真值域**：observation 不进入 Memory ledger v1、current view、projection manifest、KG、trace 或
   EventBus；它使用独立、固定的 append-only namespace 和 hash domain。
3. **同一 vault lease**：store 必须由现有 `Arc<PersonalVaultStorage>` 发放，不能第二次 open/lock vault，
   不能接受调用方提供任意宿主路径。
4. **生产默认零接线**：Phase 1A 不修改 kernel/scheduler/config，不构造 writer、不 claim namespace、
   不 ensure genesis。candidate 与 implementation base 对同一固定 lifecycle recipe 的允许 mutation 集和
   deterministic schema/catalog/semantic fixtures 等价；candidate 绝不额外产生 observation namespace、
   object、root、writer/thread/handle。只有 ledger 测试显式打开。
5. **不冒充可信身份**：Phase 1A stored record 固定携带 `attestation_state=unverified_fixture`，该值不可升级；
   future trusted record 必须使用新 schema 与 opaque trusted admission。payload/self-reported `agent_id`、
   CAS metadata、`system/default` 不能成为测试捷径，current view/summary 不得把 fixture 计作 verified evidence。
6. **不冒充 evidence 授权完成**：record 只接受 canonical-form CID 字符串引用且禁止 inline bytes；CID 存在性、完整性、
   role readability 和 commit-time authorization 由 Phase 1B 的 trusted verifier 完成，本阶段不得默认 allow
   或宣称已验证真实 evidence。
7. **一 attempt 至多一 terminal**：同一 attempt 恰好一个 Started、最多一个 Terminal；相同 canonical request/request digest 重试幂等，
   不同 terminal 是 typed conflict，不能 last-write-wins。
8. **不确定性不伪装失败或成功**：publish 后 durability 无法确认时返回
   `ObservationStoreError::CommitIndeterminate`/`Poisoned`。
   重启后的 Open attempt 必须原样保留；Phase 1A 不猜测其业务 terminal，也不自动补 Success/Failure。
   store commit uncertainty 绝不能生成或映射成 `TerminalOutcomeV1::Indeterminate`。
9. **writer-stamped**：sequence、commit time 与 root generation 由 writer 生成；调用方时间不决定顺序。
10. **不产生行为**：observation 不能创建 memory/claim/procedure/skill、改变 recall/ranking/profile/permission、
    创建任务或调用工具。
11. **无公共能力**：`plico.personal.v2` exact-14、MCP/aicli catalog 和 typed public responses 完全不变。
12. **可证伪**：任何不变量无法由自动化正例、反例、fault/restart test 证明时，里程碑为 Rejected。
13. **目录不等于语义依赖**：代码放在 `src/memory/execution_observation/` 只表示内部组织；不得 import 或复用
    Memory ledger/model/current view、LayeredMemory、projection/KG/telemetry 类型。依赖图必须由 scope gate 验证。

---

## 3. R0 待冻结的候选协议

以下是架构审议输入，不是当前开工授权。Accepted ADR 可以收窄或重命名，但不得降低不变量。

### 3.1 Attempt identity

```text
ExecutionAttemptKey
  execution_id: UUID                 # 一次逻辑 execution
  attempt: NonZeroU32                # Phase 1A 只验非零/range；不验连续、顺序或 gap
```

跨 attempt 的分配、单调性与 gap policy 属于 Phase 1B trusted producer，不得由 Phase 1A store 猜测。

禁止空串、`unknown`、随机回退 role 或把 intent ID 填进 execution ID。不是每个执行边界都有 intent；
origin ID 必须内嵌在 closed variant 中，不能另放一个可冲突的顶层 `intent_id`：

```text
FixtureOriginV1 =
  PublicRequest { request_id }
  | IntentDispatch { intent_id }
  | InternalTask { task_id }
```

Phase 1A 没有 admitted live producer；这些字段仅验证 canonical form，fixture 不代表真实生产身份。

### 3.2 Append-only events

候选协议必须把 caller request 与 immutable stored event 分层；writer stamp 不属于 retry identity：

```text
AppendStartedRequestV1
  schema, key, fixture_origin
  attestation_state = unverified_fixture
  role_ref?, session_ref?              # fixture-only opaque refs；不证明可信性
  operation_contract_sha256
  input_evidence_cids[], context_evidence_cids[]
  policy_sha256, runtime_sha256

StoredStartedEventV1
  request, request_sha256
  sequence, root_generation, recorded_at_ms

AppendTerminalRequestV1
  schema, key
  attestation_state = unverified_fixture
  outcome                             # closed TerminalOutcomeV1
  output_evidence_cids[]
  execution_elapsed_ms?               # fixture assertion；Phase 1A 只验格式/范围
  policy_sha256, runtime_sha256

StoredTerminalEventV1
  request, request_sha256
  sequence, root_generation, recorded_at_ms
```

`TerminalOutcomeV1` 的闭集固定为：

- `Success`
- `Failure { category }`
- `Timeout`
- `Cancelled`
- `Indeterminate`

`Failure` 的 category 只存在于 outcome 内，不得再有第二个 `error_category` 来源。Terminal 的
`policy_sha256/runtime_sha256` 必须与 Started 完全相等；否则是 rebind Conflict，旧 root 不变。

字段 provenance 由 R0 matrix 冻结，最低要求如下：

| 字段 | 来源 | Phase 1A 能证明什么 |
|---|---|---|
| key/origin/role/session/evidence/policy/runtime/outcome/elapsed | caller fixture | 只证明 canonical form、range 与跨记录一致性；不证明现实真实性 |
| `attestation_state` | schema 固定字面量 + writer 验证 | 永远是 `unverified_fixture`；不可升级 |
| `request_sha256` | writer 从 canonical request JCS + request domain 推导 | retry identity |
| sequence/root generation/recorded time | writer | ledger 顺序与 commit/record 时间，不是 execution 时间 |
| current view | validator 从已验证 stored events 推导 | ledger 状态，不是业务结果权威性 |

collection semantics 也由 R0 matrix 冻结：三个 `*_evidence_cids[]` 都是 **ordered list**，顺序保留并进入
request digest；同一 list 内 duplicate 拒绝，但绝不自动排序。当前 v1 没有 set 字段；未来 set 必须显式声明。

`Timeout`/`Cancelled` 的 live producer 语义不在 Phase 1A 内。测试必须证明五种值可稳定持久化，但不能
把测试 enum coverage 冒充现有 scheduler 已能正确分类。当前 blocking timeout 未来接线时默认只能归为
`Indeterminate`，除非新的 typed executor 能证明取消及副作用边界。

### 3.3 候选 schema 与 hash

- Phase 1A record schema 必须在名称上固定未验证语义，例如
  `plico.execution-observation.fixture-started/v1`、`plico.execution-observation.fixture-terminal/v1`；future
  trusted admission 必须使用新 schema，不得把 fixture 原地升级；segment/root/pointer 各自版本；
- hash 使用独立 domain separator，例如 `plico.execution-observation.fixture.v1\0`，request/event/root 域继续分离，
  不得复用 memory/projection hash；
- Structured 字节按 Accepted ADR-0004 的 RFC 8785/JCS 依赖与 golden-vector 纪律；禁止普通
  `serde_json::to_vec`、手写 key sort 或兼容 normalization；
- unknown/future schema、unknown field、非 canonical UUID/CID、控制字符、ordered-list duplicate、
  schema-declared set 未排序、时间逆序、
  sequence/size 溢出一律 typed reject；
- tracing 只记录 stable category、phase、role kind、count、elapsed；禁止正文、凭据、私有 path、完整 CID/hash。

### 3.4 Attempt state machine

```text
Absent
  └─ append Started ──> Open
Open
  ├─ same Started request retry ───> same receipt / no generation change
  ├─ different Started request ────> Conflict / old root unchanged
  ├─ append at most one Terminal ──> Terminal
  └─ restart ──────────────────────> Open（字节/sequence 不变）
Terminal
  ├─ same Terminal request retry ──> same receipt / no generation change
  └─ different Terminal request ──> Conflict / old root unchanged
```

幂等比较只使用 caller-controlled canonical request digest；命中既有 event 时必须返回第一次保存的 receipt，
不得重取时间、重分 sequence 或增加 generation。同 key 下 Started 任一 caller 字段变化均 Conflict；Terminal
必须引用相同 Started key，且 policy/runtime 不得重绑定。

`Started` 必须 durable 后才算 ledger accepted。Phase 1A 不接真实业务，因此不定义 begin-before-action、
coverage 或 recovery terminal policy；Open 在重启后保持 Open，绝不由 store 自行解释为成功、失败、取消
或超时。Phase 1B 才能在可信执行边界上决定何时追加 `Indeterminate`。

---

## 4. 允许与禁止修改范围

### 4.1 Developer implementation allowlist（只有 R0 Accepted 后）

| 路径 | 允许变更 |
|---|---|
| `src/memory/execution_observation/**` | `pub(crate)` model、hash、validator、current view、store、测试；单文件目标 `<300` 行 |
| `src/cas/ledger_store.rs` | 只增加一个固定 namespace 与同一 lease 下的 exact opener；不得泛化任意 path API |
| `src/cas/mod.rs` | 最小 crate-private re-export |
| `src/memory/mod.rs` | 只注册 crate-private observation ledger module |
| `src/cas/INDEX.md`、`src/memory/INDEX.md` | 更新 dependency、ownership、risk 与 invariant |
| `AGENTS.md` | 仅新模块导航和架构事实 |
| `src/memory/execution_observation/**` 内联测试、`tests/execution_observation_*.rs` | model/hash/store/restart/fault/zero-mutation 证明 |

没有 Accepted R0 ADR 时，上表所有生产路径仍禁止修改。

### 4.2 Architecture-owned pre-freeze files

以下文件可由架构/QA 在 R0 freeze 前创建或修改，但进入 implementation base 后必须逐文件 digest 固定，
developer diff 一律拒绝：窄化 ADR、本合同、`v53-summary.md`、milestone INDEX/next-era plan，以及
`scripts/milestones/v53/{collect,verify,verify_scope}.*`。开发组只可执行 frozen tools、读取合同并在外部
evidence bundle 提供输入；不得修改这些文件。后续 summary/INDEX 状态只能由 reviewer 在独立签名提交中
更新，不属于 developer candidate diff。

`verify_scope` 必须相对 frozen implementation-base 检查 `git diff --name-status`：任何不在 exact allowlist 的
新增、修改、删除、rename、symlink/submodule 变化都非零退出；特别禁止 `Cargo.toml`、`Cargo.lock`、`build.rs`、
`.cargo/**`、feature/workflow/macro 侧门。它还必须扫描 observation 模块不得 import Memory ledger/model/current
view、LayeredMemory、projection/KG/telemetry。开发组不能修改 scope gate 本身。

### 4.3 无条件禁止

- `src/api/public/**`、`src/kernel/**`、`src/scheduler/**`、`src/client.rs`；
- `src/bin/**`、MCP/aicli mapping、PUBLIC_OPERATIONS exact-14 catalog；
- Memory ledger v1 schema/hash/current-view/writer 与 Projection Manifest schema；
- `kernel/cognition/trajectory_tracker`、`experience_miner`、SkillForge/SkillRegistry；
- prefetch feedback、profile、KG、vector/BM25、summary、thermal/retrieval policy；
- trace/event/cost/session/task JSONL 升格为 observation truth；
- permission 默认值、trusted bypass、self-reported role；
- checkpoint/fork/rollback/replay、procedure/claim/feedback learning；
- serving framework、embedding/LLM/VLM 模型或 provider；
- 新 public operation、reader/list/history/export endpoint；
- 新第三方依赖、兼容 wrapper、dual write 或任意路径 namespace，除非架构组重新立 ADR。

触碰禁止区即停止当前工作包，提交 architecture escalation；不得用 helper、feature alias 或 deprecated
adapter 绕过。

---

## 5. 第三方工作包

依赖顺序不可并行跨越：

```text
WP0 → WP1 → WP2 → WP3 → WP4 → WP5 → WP6
```

每个 WP 独立 PR；架构 checkpoint 未签字前，不得把下一 WP 堆入同一 PR。

| WP | 负责人 | 交付物 | 自动化验收 | 硬停止条件 |
|---|---|---|---|---|
| WP0 合同冻结 | 架构组 | Accepted 窄 ADR、exact 字段/limits、provenance/collection matrix、namespace/hash/store topology、Phase 1B blocker、error taxonomy、toolchain、scope/evidence 工具 | R0 checklist + schema/golden/tool digest 双审；implementation base clean | ADR 未 Accepted；未来身份/权限边界未显式列为 blocker；工具不可机械验证；需要 public operation |
| WP1 纯类型/验证 | 开发组 | typed IDs、Started/Terminal/outcome、strict validator、JCS/domain hash、typed error；不含 I/O | round-trip、golden vector、future schema、非法 ID/CID、unknown field、重复、时间/数值溢出 | 用 `serde_json::Value` 保存正文；import/reuse Memory ledger/model/current view、KG/LLM；出现 public export |
| WP2 CAS-owned store | 开发组 | fixed namespace、append-only segments/root、stable receipt、single writer、known-key read、startup structural validation | genesis/append/root generation/restart byte equality/future-schema；仅验证 store 结构，不提前实现 attempt 语义 | 第二把 vault lock；任意 path；改 memory ledger；publish 未确认却 success |
| WP3 Current view | 开发组 | 从 immutable events 重建 attempt 状态；Open/Terminal 唯一视图；五种 typed terminal 持久化 | duplicate start、terminal-without-start、same/different terminal、concurrent terminal、restart Open/Terminal equality | 自动把 Open 归类 terminal；解析 bool/String；引入 kernel producer |
| WP4 fault/recovery | 开发组 | `ObservationStoreError::{Rejected,Conflict,CommitIndeterminate,Poisoned}`、fault points、restart full validation | pre/post publish、sync indeterminate、writer poison、restart、race、segment/root/pointer/view tamper | 半提交报 success；两个 terminal；恢复信任未验证 bytes；日志泄漏 |
| WP5 不变性回归 | 开发组 + QA | exact-14 before/after、machine scope gate、base/candidate fixed-lifecycle differential、Rust/Python gates | public/MCP parity、零 observation 额外资源、全 Rust gate、benchmark pytest/ruff | public surface 或 benchmark evaluator 语义改变；任一 gate 红 |
| WP6 候选交付 | 开发组 | clean candidate SHA、供验收组填写 summary 的证据输入、sealed evidence manifest、fault matrix、review checklist；不得编辑 summary/合同状态 | fresh vault 独立重跑；manifest/digest/COMMITTED 完整 | dirty tree、缺证据、真实个人数据、不可复现、开发组自称 Accepted |

---

## 6. 最小反例与故障矩阵

全部用自动化测试实现；“人工验证”“日志看起来正确”不算完成。

| ID | 场景 | 必须结果 |
|---|---|---|
| F01 | implementation base 与 candidate 执行同一 fixed fresh/existing lifecycle recipe | 允许 mutation 集和 deterministic schema/catalog/semantic fixture 等价；candidate 无 observation namespace/object/root/thread/handle |
| F02 | 相同 Started/Terminal canonical request 重试 | 返回首次 receipt/stamp；root generation 不增长 |
| F03 | 同 key 改任一 Started 字段，或并发提交不同 Started | typed Conflict；只保留一个 immutable Started，旧 root 不被覆盖 |
| F04 | 同 key 改 Terminal/outcome/evidence，或 policy/runtime 与 Started 不同 | typed Conflict；旧 root/bytes 不变，禁止 evidence/policy rebind |
| F05 | 两线程竞争相同/不同 terminal | 相同请求幂等；不同请求只发布一个，另一方 conflict；无双 head |
| F06 | segment/object 写后、root publish 前 crash | orphan 可存在但不可见为 active observation；无假 accepted |
| F07 | Started 后重启 | current view 仍为同一 Open；不自动追加任何 terminal |
| F08 | Terminal 后重启 | bytes/hash/sequence/current view 完全一致；五种 outcome 逐类覆盖 |
| F09 | root pointer exchange 后、parent fsync 前失败 | 返回 `ObservationStoreError::CommitIndeterminate`/`Poisoned`；restart 唯一恢复，不返回 success，且不生成业务 terminal |
| F10 | malformed/noncanonical CID 或 inline bytes 字段 | Rejected；root 不变 |
| F11 | 测试尝试用 CAS metadata/self-reported role 建授权结论 | 只可落 `unverified_fixture`；不存在 verified API/断言，Phase 1B blocker 保持显式 |
| F12 | 显式打开 observation store 后 tamper segment/root/pointer/current view | observation startup fail closed；用独立实例验证 Memory ledger/lexical 仍可读，不接普通 AIKernel startup |
| F13 | UUID/CID 控制字符、未知字段/schema、非 JCS、>2^53、时间逆序 | strict typed reject；零写入 |
| F14 | bearer/secret/raw error/path 注入 | tracing/error/evidence bundle 扫描零泄漏 |
| F15 | Memory/KG/projection/skill/profile 状态差分 | base/candidate 固定 recipe 语义等价；没有 observation 额外写入、自动学习或排名变化 |
| F16 | exact-14 catalog 与 MCP descriptor 对比 | 集合、顺序和 schema golden 完全一致 |

测试名固定为 `execution_observation_f01_*` … `execution_observation_f16_*`；至少执行
`cargo test --lib execution_observation_f` 和相应 integration target。verifier 必须逐 ID 证明至少一个实际执行的
test，并将 `ID → test name → result` 写入 manifest；任一 ID 为 0 或 aggregate filter 匹配 0 都是失败。

### 6.1 Five-terminal 正例

`Success / Failure / Timeout / Cancelled / Indeterminate` 必须分别有 schema、hash、restart round-trip 与
幂等测试。不得以一个 string error fixture 覆盖五类。

### 6.2 性能边界

Phase 1A 没有 live producer，因此不宣称请求延迟收益。只测 store micro-boundary：固定本机、固定
fixture、预热与 unique append 分开，报告 p50/p95/吞吐和 bytes/record；结果只作回归基线，不作竞品结论。
普通生产路径以代码路径和 observation 额外资源均为零验收；store microbench 只报告，不设 GO 阈值，除非
R0 冻结 workload、warmup、样本数和阈值。出现常驻 writer/thread、额外目录扫描或 namespace genesis 即
直接 no-go。

---

## 7. Review checkpoints

| Checkpoint | 审查方 | 必须签字内容 | 未通过时 |
|---|---|---|---|
| R0 Constitution/ADR | 架构组 | Accepted narrow ADR、非目标、production-zero-wiring/lazy-init、namespace/hash/owner | 禁止代码开工 |
| R1 Model/Hash | 架构 + 数据完整性专家 | schema、limits、JCS、typed terminal、golden vectors | WP1 退回；不得写 store |
| R2 Store Core | 架构 + 存储专家 | same vault lease、fixed namespace、root chain、atomic publish、restart byte equality | WP2 退回；不得进入 current view/fault 工作包 |
| R3 View/Identity boundary | 架构 + 安全专家 | attempt state/idempotency/conflict；无 live identity/evidence-completeness claim；Phase 1B blocker 明确 | WP3 退回 |
| R4 Fault/Admitted boundary | 架构 + 存储/安全专家 | poison/tamper/fault；Phase 1A 无 live producer、kernel/scheduler diff 为零 | WP4 退回；runtime hook 必须删除 |
| R5 Adversarial QA | 独立 QA + 安全专家 | F01–F16、fault/restart/race/privacy | 任一失败均 NO-GO |
| R6 Final alignment | 架构组 | public exact-14、Memory/projection truth、bench/Rust green、evidence complete | Accepted 或 Rejected |

允许的最终裁决只有：

- `GO`：全部不变量和证据完整；
- `NO-GO`：有未满足的 P0/P1 或证据缺口；
- `GO with named debt`：只允许不影响权限、durability、truth、public API 的 P2 债务，并给 owner/date。

---

## 8. 质量门禁

### 8.1 产品基线与冻结工具链

产品基线 commit 已执行：

```bash
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib
# 2046 passed; 0 failed; 2 ignored（2026-08-16 本机基线）
```

数字只绑定该次 commit/环境，不是永久质量承诺。第三方必须在自己的 clean checkout 重跑并保存输出。
它不是 implementation base；R0 必须生成一个包含 Accepted ADR、冻结合同、scope/evidence 工具的 clean
implementation-base SHA。当前观测工具链为 Rust/Cargo 1.95.0、cargo-llvm-cov 0.8.6、uv 0.11.14；R0
必须把最终 exact version 和安装来源写入 handoff packet，版本不符即停止而不是自行升级。

### 8.2 每个 WP 的最小命令

```bash
cargo fmt --all -- --check
cargo check --all-targets --all-features
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib --all-features
cargo clippy --all-targets --all-features -- -D warnings
./scripts/milestones/v53/verify_scope.sh --base <FROZEN_IMPLEMENTATION_BASE_SHA>
git diff --check
```

`<FROZEN_IMPLEMENTATION_BASE_SHA>` 是外部 R0 handoff packet 提供的 metavariable，不在合同内回填，避免
Git commit self-reference；handoff 必须给出展开后的 exact invocation、packet digest 与架构签名。scope gate
非零退出或匹配 0 个 observation tests 都是失败。

### 8.3 Candidate 全门禁

```bash
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib --fail-under-lines 87
cargo build --release --all-features
cargo test api::public
cargo test catalog_is_the_exact_service_surface
cargo test --test mcp_test mcp_catalog_is_the_exact_public_catalog

cd benchmarks
uv sync --locked --extra dev
uv run ruff check src tests
uv run ruff format --check src tests
uv run pytest -q
```

若命令名与当前仓库实际 test target 不匹配，开发组必须报告 Architecture Deviation；不得删除门禁、
换成手工结果或添加“总能通过”的空测试。所有 filtered `cargo test` 输出必须封存实际匹配/执行数量；
0-test green 视为失败。

产品基线实测上述三个 public filter 分别执行 `10 / 1 / 1` 个测试；R0 collector 必须封存 base/candidate
实际计数，不能只保存 exit code。

---

## 9. 交付证据包

证据包位于源码仓库外或被明确 ignore 的 owner-only 目录；源码仓只提交 `v53-summary.md` 中的 digest、
计数和结论，不提交本机绝对路径、真实正文、凭据、binary 或不完整日志。

```text
v53-evidence/
├── manifest.json
├── developer-handoff.json
├── source/
│   ├── product-baseline-sha.txt
│   ├── implementation-base-sha.txt
│   ├── candidate-sha.txt
│   ├── toolchain.txt
│   ├── platform.txt
│   └── dirty-digest.txt
├── contracts/
│   ├── accepted-adr.sha256
│   ├── observation-v1-golden.json
│   ├── public-operations-before.json
│   └── public-operations-after.json
├── tests/
│   ├── rust-lib.txt
│   ├── rust-all.txt
│   ├── clippy.txt
│   ├── fmt.txt
│   ├── coverage.txt
│   ├── perf-regression.txt
│   └── benchmark-pytest.txt
├── invariants/
│   ├── production-zero-wiring.json
│   ├── idempotency.json
│   ├── conflict.json
│   ├── restart-replay.json
│   ├── fault-matrix.json
│   ├── privacy-scan.json
│   └── forbidden-diff.txt
├── detached.sha256
└── COMMITTED
```

约束：

- manifest 绑定 product baseline、implementation-base、candidate SHA、dirty=false、Cargo.lock、benchmarks/uv.lock、toolchain、OS、feature set；
- manifest schema、canonical digest 算法、collector/verifier version 与 exact 命令由 R0 架构工具冻结；
- 所有文件先写完、校验并 fsync，`COMMITTED` 最后生成；
- schema/golden、fault matrix 与 public snapshot 必须有独立 digest；
- 不记录 bearer/token、正文、query、response、私有 host path、完整低熵内容 hash；
- 一个 demo、单元测试数量或 coverage 百分比不能单独构成 acceptance evidence。
- 开发组只能通过 frozen collector 生成 `developer-handoff.json` 和原始输入；只有 frozen verifier 非零测试数、
  digest、COMMITTED、权限与 schema 全通过且 exit 0，状态才可进入 Evidence-Complete。禁止手工拼装 sealed bundle。

---

## 10. 第三方交付与架构 Review 流程

1. 架构组冻结 R0，提供 Accepted ADR/contract digest 与 external handoff 中的 implementation-base SHA。
2. 架构组创建并保护 `v53-integration`；开发组按 WP0→WP6 提交 stacked PR，只合入该集成分支。
   R6 前禁止进入 `main`；PR 描述只陈述本包差异、测试和未决项，不修改目标/门禁。
3. 开发组提交自验证结果，但不能填写 Accepted、不能将 warning/ignored test 解释为通过。
4. QA 在 clean checkout + fresh vault 独立重跑 golden、fault、restart、race 和 regression。
5. 安全专家审查未实现的 role/evidence 能力没有被冒充完成，并复核日志/错误、任意 path、第二把 vault lock 与 fail-open。
6. 架构组审查依赖方向、single truth、exact-14、production-zero-wiring/lazy-init 和 forbidden diff。
7. 架构/专家共同给出最终裁决；只有架构组更新 summary 和 INDEX。

### 提交规范

- 不允许 `git add .`；精确暂存 allowlist 文件。
- 不提交 `.runtime/`、`.logs/`、benchmark result、真实 vault、trace、binary、cache、`__pycache__`。
- 不做 drive-by refactor、格式化全仓、升级依赖或改 unrelated test expectation。
- 遇到上位规范冲突、必须触碰 denylist、身份来源不可信或 store 需要第二路径时，立即停止并报告。

---

## 11. 总体验收与停止条件

### 11.1 Accepted 必须全部满足

- [ ] R0–R6 均由规定审查方签字；
- [ ] Accepted 窄 ADR digest 与代码/证据包一致；
- [ ] WP0–WP6 全部完成且每包可独立 review；
- [ ] F01–F16 全部自动化通过，且 filter 匹配数非零；
- [ ] base/candidate 同 lifecycle recipe 的允许 mutation 与 deterministic fixture 等价；candidate 零 observation namespace/object/root/thread/handle；
- [ ] same attempt 幂等、conflict、race、indeterminate、poison/restart 语义完整；
- [ ] record 只含 canonical-form CID 字符串引用、无 inline bytes；没有虚假 evidence existence/authorization 声明；
- [ ] public exact-14 与 MCP/aicli/catalog golden 未变；既有 deterministic response fixtures/语义未变；
- [ ] Memory ledger、projection、KG、skill/profile/retrieval 无写入或行为变化；
- [ ] Rust/Python/coverage/perf/format/clippy/diff 全绿；
- [ ] clean candidate、sealed evidence、independent replay 完整；
- [ ] `v53-summary.md` 只记录可复算事实和诚实限制。

### 11.2 任一发生即 NO-GO / Rejected

- 没有 Accepted R0 ADR 就创建 durable namespace/writer；
- 通过解析 bool/string/log 推断 authoritative terminal；
- timeout/abort 后仍可能执行却标 Cancelled/Failure；
- self-reported role、`system/default` 或 `AIObjectMeta` 被当作授权证明或被宣称为 credential-bound；
- observation 保存正文/凭据，或 Phase 1A 宣称已经完成 evidence existence/readability/authorization；
- 普通 kernel/daemon 路径额外初始化 observation 目录、claim namespace、启动 writer/thread/handle，或改变
  exact-14/catalog golden/既有 deterministic response semantics；
- terminal 半提交、冲突覆盖、两个 active terminal、publish 不确定却返回 success；
- 复用 Memory/KG/trace/EventBus/feedback 作为 observation truth；
- 观察结果进入 learning、ranking、skill、claim、permission、task/tool action；
- 新增 public operation、兼容层、dual write 或 serving 依赖；
- 缺失 fault/restart/race/privacy 证据，或证据绑定 dirty/错误 commit。

---

## 12. 完成后边界

即使 v53 Accepted，Plico 仍只拥有一个 **internal, unconnected observation ledger core**：

- 没有 kernel/scheduler recorder、trusted execution context 或真实 producer coverage；
- 没有 commit-time evidence existence/readability/authorization adapter；
- 没有 Open attempt 自动收敛策略；
- 没有 public query/list/history；
- 没有 feedback/procedure/skill learning；
- 没有 branch/checkpoint/rollback/replay capability；
- 没有 Verified Experience product gate；
- ADR-0006 仍需按后续阶段独立审议。

下一里程碑必须先冻结一个真实、credential-bound 的 execution boundary 与 begin-before-action 顺序，
然后才能接入单一 producer。不得把 v53 的存储通过“顺手 hook scheduler”扩成全链路能力。
