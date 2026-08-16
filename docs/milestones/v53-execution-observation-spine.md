# v53 里程碑：Execution Observation Ledger Core（Phase 1A）

**日期**：2026-08-16
**合同版本**：`plico.milestone.v53/1`
**状态**：R0 Freeze Candidate / Implementation not started
**产品基线**：`fe4c08260fc3e6dc0e3d37921b863a7ed48a330a`
**Architecture-Frozen identity**：只有在外部 R0 packet 完整性通过、独立 Git 审批提交与派生标签均通过
离线授权器验证后才成立；本合同不写自身 commit SHA，以避免 self-reference
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
- 进入 `Architecture-Frozen` 后，本合同正文不可原地修改；冻结身份/状态由外部 R0 packet 与独立 Git 审批提交/标签共同绑定，
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
逐字段 provenance/collection-semantics matrix、crate-private `open/append/read` exact 签名、private rebuild
合同与文件清单、
架构组拥有的 R0 packet/scope 工具、固定工具链版本，以及 fresh/existing lifecycle differential recipe。该 recipe
必须冻结操作序列、deterministic fixture/normalization、base 允许 mutation 集、observation absence 判定、
比较器命令与退出语义。

### R0 必须闭合的基线缺口

R0 implementation base 必须已经移除已删除 `plico-sse` 的陈旧构建门，并把 benchmark Python
pytest/ruff 纳入本地门禁。`Cargo.lock` 必须成为 Git tree 的受控输入，Rust/Cargo/Git/coverage 工具均固定
identity，所有会解析依赖的 Cargo 命令使用 `--locked`。这些本地命令、工具和 lock bytes 在 handoff 中逐项绑定；第三方不得
顺手修改它们。只有 `--locked` 时只能声明依赖解析受 lock 约束，不能宣称 dependency source/cache 或整个
构建环境完全可复现。

GitHub 在 v53 只用于 Git 分支、提交、标签、PR 与人工 review 协作；禁止 GitHub Actions、Issues、托管 check、
GitHub API 授权和任何收费外部 gate。所有构建、测试、coverage、packet 与 scope 验证都在本地执行；本合同不把
平台在线状态或同名 check 当作授权证据。

### R0 handoff 的信任边界

R0 packet 的 attestation kind 固定为 `unsigned_repository_control_attestation`，不能称为 cryptographically
signed。四文件 packet 只证明所审 bytes 的完整性，`authorization` 永远是 `unverified`。架构授权必须另有一个
直接以 implementation base 为唯一父提交、且只新增固定 canonical approval record 的 Git 提交 A；派生标签
`v53-r0-<COMMITTED_SHA256>` 必须精确指向 A。离线授权器同时绑定 base/tree、packet、合同、Accepted ADR、scope
工具、`Cargo.lock`、toolchain、审批时效和人工 reviewer 声明，并把 A 作为第三方 candidate scope base。
该程序性机制不提供密码学 reviewer 身份或不可抵赖性，也不抵抗仓库管理员改写历史、tag/ref 或 same-UID/host
失陷；任一 record/ref/tag/digest/time 无法复核时必须 fail closed，不得进入 Architecture-Frozen。

R0 外部 packet 固定只有四个 owner-only regular files：`LOCK`、`handoff.json`、
`handoff.sha256.json`、`COMMITTED`。`COMMITTED` 最后写入；缺失、额外文件、symlink/special file、权限不私有、
digest/commit/tree/工具/lock 不匹配全部 fail closed。四文件 packet 单独不授权开工；只有上述 A/tag 离线验证
为 GO 后，才授权从 A 开始 WP1。它不是 WP6 candidate evidence bundle。

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

### 4.1 Developer implementation allowlist（按 checkpoint 逐级解锁）

| 路径 | 允许变更 |
|---|---|
| R0 → WP1 | 只允许 `src/memory/mod.rs` 新增一次精确的 crate-private module declaration（除此之外字节不变），以及 R0 spec 明列的 `src/memory/execution_observation/{mod.rs,ids.rs,model.rs,canonical.rs,hash.rs,validation.rs,error.rs,tests.rs}`；只做纯类型/JCS/hash/validator/self-tests，无 I/O、无 `crate::cas`/Memory/current-view 能力 |
| R1 → WP2 | 只有 R1 接纳 WP1、冻结架构组外部 corpus 并发布新 scope spec/approval 后，才可增加 store 文件与 `src/cas/ledger_store.rs`、`src/cas/mod.rs` 的 exact 最小 anchor |
| R2 → WP3 | 只有 R2 接纳 store 后，才可增加 current-view/rebuild 文件 |
| R3 → WP4 | 只有 R3 接纳状态机后，才可增加 fault/recovery 文件与相应用例 |
| R4 → WP5/WP6 | 只有 R4 接纳 fault/recovery 后，才可执行全回归和 sealed candidate evidence 工作包；INDEX 由架构组在接纳 checkpoint 后更新 |

没有 Accepted R0 ADR 时，上表所有生产路径仍禁止修改。当前 `plico.v53.r0-spec/v1` 和审批提交 A **只授权
WP1**；`verify_scope.py` 必须拒绝 `WP2..WP6`，也必须拒绝 WP1 修改 CAS、store、view、fault、INDEX 或任意未列路径。
后续工作包必须使用新版本 scope spec 与新的架构审批，不得复用 R0 A/tag 越过 checkpoint。

### 4.2 Architecture-owned pre-freeze files

以下文件可由架构/QA 在 R0 freeze 前创建或修改，但进入 implementation base 后必须逐文件 digest 固定，
developer diff 一律拒绝：窄化 ADR、本合同、`v53-summary.md`、milestone INDEX/next-era plan，以及
`scripts/milestones/v53/{r0_spec.json,collect.py,verify.py,verify_scope.py,authorize.py,test_v53_tools.py,test_v53_authorize.py}`、
`.gitignore`、`rust-toolchain.toml`、`Cargo.lock`、`AGENTS.md` 与独立 approval record。开发组只可执行 frozen tools、
读取合同并在外部 evidence bundle 提供输入；不得修改这些文件。后续 summary/INDEX 状态只能由 reviewer 在独立提交中
更新，不属于 developer candidate diff。

新模块导航所需的 `AGENTS.md` 最终更新由架构组在 R6 根据已接纳事实单独完成；开发组不得借导航更新扩大
R0 scope。

`verify_scope.py` 的正式入口必须在同一进程先执行离线 Git 授权，取得审批提交 A 作为 frozen candidate base；
packet-only、调用方自选 base 或只跑 packet integrity verifier 都不得进入 scope。随后检查
Git object tree 的 NUL-delimited raw diff：任何不在 exact allowlist 的
新增、修改、删除、rename、symlink/submodule 变化都非零退出；特别禁止 `Cargo.toml`、`Cargo.lock`、`build.rs`、
`.cargo/**`、feature/hosted-workflow/macro 侧门。它还必须扫描 observation 模块不得 import Memory ledger/model/current
view、LayeredMemory、projection/KG/telemetry。untracked/ignored/index-only/sparse-checkout 异常同样拒绝；开发组
不能修改 scope gate 本身。该静态门是最小范围证明，不冒充完整 Rust 语义分析；架构 review 和编译测试仍是
独立门禁。

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
| WP0 合同冻结 | 架构组 | Accepted 窄 ADR、exact 字段/limits、provenance/collection matrix、namespace/hash/store topology、Phase 1B blocker、error taxonomy、toolchain、packet/scope 工具 | R0 checklist + schema/golden/tool digest 双审；implementation base clean；独立 A/tag 离线授权 | ADR 未 Accepted；未来身份/权限边界未显式列为 blocker；工具不可机械验证；需要 public operation |
| WP1 纯类型/验证 | 开发组 | typed IDs、Started/Terminal/outcome、strict validator、JCS/domain hash、typed error；不含 I/O | round-trip、golden vector、future schema、非法 ID/CID、unknown field、重复、时间/数值溢出 | 用 `serde_json::Value` 保存正文；import/reuse Memory ledger/model/current view、KG/LLM；出现 public export |
| WP2 CAS-owned store | 开发组 | fixed namespace、append-only segments/root、stable receipt、single writer、known-key read、startup structural validation | genesis/append/root generation/restart byte equality/future-schema；仅验证 store 结构，不提前实现 attempt 语义 | 第二把 vault lock；任意 path；改 memory ledger；publish 未确认却 success |
| WP3 Current view | 开发组 | 从 immutable events 重建 attempt 状态；Open/Terminal 唯一视图；五种 typed terminal 持久化 | duplicate start、terminal-without-start、same/different terminal、concurrent terminal、restart Open/Terminal equality | 自动把 Open 归类 terminal；解析 bool/String；引入 kernel producer |
| WP4 fault/recovery | 开发组 | `ObservationStoreError::{InvalidRequest,TransitionConflict,LimitExceeded,CorruptStore,StorageUnavailable,NamespaceAlreadyClaimed,CommitIndeterminate,Poisoned}`、fault points、restart full validation | pre/post publish、sync indeterminate、writer poison、restart、race、segment/root/pointer/view tamper | 半提交报 success；两个 terminal；恢复信任未验证 bytes；日志泄漏 |
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
| F06 | segment/object/candidate 写后、root exchange 前 crash | active 仍 authoritative；candidate 仅可为其 exact direct child 且不得自动 promote；orphan 不可见为 accepted |
| F07 | Started 后重启 | current view 仍为同一 Open；不自动追加任何 terminal |
| F08 | Terminal 后重启 | bytes/hash/sequence/current view 完全一致；五种 outcome 逐类覆盖 |
| F09 | root pointer exchange 后、parent fsync 前失败 | 返回 `ObservationStoreError::CommitIndeterminate`/`Poisoned`；poison 后读写全拒；restart 双槽全验后按 active 恢复，不返回 success，且不生成业务 terminal |
| F10 | malformed/noncanonical CID 或 inline bytes 字段 | Rejected；root 不变 |
| F11 | 测试尝试用 CAS metadata/self-reported role 建授权结论 | 只可落 `unverified_fixture`；不存在 verified API/断言，Phase 1B blocker 保持显式 |
| F12 | 显式打开后 tamper segment/root/active/candidate/current view；同 root、非直接父子、空槽非法组合 | observation startup fail closed；合法旧 pair 只证明内部一致、不得宣称 authenticated freshness；用独立实例验证 Memory ledger/lexical 仍可读，不接普通 AIKernel startup |
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
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --lib
# 2046 passed; 0 failed; 2 ignored（2026-08-16 本机基线）
```

数字只绑定该次 commit/环境，不是永久质量承诺。第三方必须在自己的 clean checkout 重跑并保存输出。
它不是 implementation base；R0 必须生成一个包含 Accepted ADR、冻结合同、packet/scope 工具的 clean
implementation-base SHA。当前观测工具链为 Rust/Cargo 1.95.0、cargo-llvm-cov 0.8.6、uv 0.11.14；R0
必须把最终 exact version 和安装来源写入 handoff packet，版本不符即停止而不是自行升级。

### 8.2 R0 授权的 WP1 最小命令

```bash
CARGO_NET_OFFLINE=true
export CARGO_NET_OFFLINE
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --lib --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cd benchmarks && uv run python ../scripts/milestones/v53/verify_scope.py \
  --handoff-dir <R0_HANDOFF_DIR> --repo .. \
  --approval-commit refs/remotes/origin/v53-integration --candidate HEAD \
  --work-package WP1 --require-clean
git diff --check
```

`<R0_HANDOFF_DIR>` 是外部 R0 handoff packet 路径，不在合同内回填，避免 Git commit self-reference；handoff
必须给出 implementation base、展开后的 exact invocation、packet digest 与 unsigned repository-control
attestation。正式 scope 入口必须在同一进程先验证 packet，再验证 approval commit/tag 并用该 approval commit
作为 base；直接调用 packet verifier 只能得到 `integrity=verified, authorization=unverified`。scope gate
非零退出或匹配 0 个 observation tests 都是失败。

`verify_scope.py` 不能把源码中的函数名计作执行证据。它固定运行
`cargo test --locked --all-features execution_observation_f -- --list`，再把当前 WP 及此前 WP 的每个列出名称用
`--exact --nocapture` 独立执行，并要求唯一 `... ok` 与 `1 passed / 0 ignored` summary；普通函数、`cfg` 未编译
用例、`ignored`、伪造输出和 0-test 均失败关闭。
当前 R0 scope 只接受 `WP1`；任何 `WP2..WP6` 输入必须非零退出。后续 checkpoint 只能由新版本 spec、packet 与
审批提交逐级解锁，不能复用 R0 A/tag。

这些由 candidate 自己提供并执行的 F 用例只属于 **candidate self-evidence**，不能单独构成任何 R checkpoint
验收结论。R0 四文件包只授权 WP1 开发；在 R1 及后续对应 checkpoint 之前，架构/QA 必须另行提供并绑定
architecture-owned external corpus/runner，开发组不能修改或用自身测试替代。external corpus 尚未绑定时只能记录
实现候选的自证结果，不得标记 R1–R6 GO。
未来 R4 授权的 WP5/WP6 verifier 还必须从 Git object archive 分别建立 base/candidate clean checkout，构建同一
`aicli` 入口并执行 fresh→memory fixture→进程重启→existing recall 固定 recipe；normalized semantic
responses、owner-only mutation inventory 必须相同，且运行中 `/proc` thread/fd 采样和落盘 inventory 都不得
出现 `execution-observation-fixture-ledger`。任一 checkout 无法构建/执行、输出不是 strict JSON、inventory
无法读取或比较器缺证据时均非零退出；开发组不能提供自制 lifecycle report 替代该独立重跑。
scope tool 还会把 absolute Cargo launcher、realpath、launcher/resolved-1.95 binary SHA 与 R0 packet 比对，
以最小 `PATH` 和清理后的 Cargo/Rust/linker 环境执行并在命令前后复核 digest；这只是程序性工具身份绑定，
不声称抵抗能够在检查与执行之间改写同一文件的 same-UID 或 host compromise。
当前 R0 本地 runner 通过私有 HOME/CARGO_HOME、offline Cargo、绝对 Git/Cargo/tool realpath 与 digest、命令后
HEAD/clean/replace-ref 复核提供程序性隔离；它不是 OS 安全边界。能够敌对控制同一 UID/宿主的场景必须在 R1 前
切换到架构拥有的只读 candidate archive、禁网、独立 UID/namespace runner，否则 fail closed，不得出具验收结论。

### 8.3 Candidate 全门禁

```bash
CARGO_NET_OFFLINE=true
export CARGO_NET_OFFLINE
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --test perf_regression
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --locked --lib --all-features --fail-under-lines 85
cargo build --locked --release --all-features --bins
cargo test --locked api::public
cargo test --locked catalog_is_the_exact_service_surface
cargo test --locked --test mcp_test mcp_catalog_is_the_exact_public_catalog

cd benchmarks
uv sync --locked --offline --extra dev
uv run ruff check src tests
uv run ruff format --check src tests
uv run pytest -q
```

若命令名与当前仓库实际 test target 不匹配，开发组必须报告 Architecture Deviation；不得删除门禁、
换成手工结果或添加“总能通过”的空测试。所有 filtered `cargo test` 输出必须封存实际匹配/执行数量；
0-test green 视为失败。

R0 实测 all-features 全仓行覆盖率为 `85.83%`（`54,742 / 63,776`）；原 `87%` 门因此天然失败，不能作为
绿色本地 gate。R0 把整数硬底线校正为 `85%`，但 v53 candidate 的差分门更严：
全仓精确覆盖率不得低于冻结基线 `85.83%`，且 `src/memory/execution_observation/**` executable line coverage
不得低于 `95.00%`。WP5/WP6 必须用 frozen scope verifier 解析 LCOV 并同时验证两个阈值；只通过本地 85%
整数门不构成 v53 接纳证据。

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
- 上述目录是目标形状，不是 R0 已交付的工具承诺；candidate evidence manifest schema、canonical digest 算法、
  collector/verifier version、架构组外部反例语料与 exact 命令必须在 R1 接纳 WP1 前冻结并 packet-bind；
- 所有文件先写完、校验并 fsync，`COMMITTED` 最后生成；
- schema/golden、fault matrix 与 public snapshot 必须有独立 digest；
- 不记录 bearer/token、正文、query、response、私有 host path、完整低熵内容 hash；
- 一个 demo、单元测试数量或 coverage 百分比不能单独构成 acceptance evidence。
- R1 evidence 工具未冻结前，开发组只能提交 candidate self-evidence，不能声称 Evidence-Complete，也不能解锁
  WP2。冻结后开发组只能通过该 collector 生成 `developer-handoff.json` 和原始输入；只有 verifier 对架构组语料、
  非零测试数、digest、COMMITTED、权限与 schema 全通过且 exit 0，状态才可进入 Evidence-Complete。禁止手工拼装 sealed bundle。

---

## 10. 第三方交付与架构 Review 流程

1. 架构组生成 R0 packet，在只新增 canonical approval record 的独立提交 A 上做人工 review，并创建派生 tag；
   离线授权器验证 A/tag 后才冻结 R0，返回 A 作为 candidate scope base。
2. 架构组创建 `v53-integration`；团队流程要求它只接收经过人工 review 的 PR。GitHub 仅承担 Git/PR 协作，
   不启用 Actions、Issues、托管 checks 或外部授权 API。开发组按 WP1→WP6 提交 stacked PR，只合入该集成分支。
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
