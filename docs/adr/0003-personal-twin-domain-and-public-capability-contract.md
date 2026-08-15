# ADR-0003：个人数字分身统一领域模型与公共能力契约

- 状态：Accepted（V1-A typed public cutover 已实现；旧内部命令与测试清债继续）
- 日期：2026-08-13
- 上位决策：[ADR-0001：个人数字分身与记忆原生数据模型](./0001-personal-digital-twin.md)
- 数据与腐败策略：[ADR-0002：Canonical Memory、按需投影与可逆检索腐败](./0002-canonical-memory-and-reversible-retrieval-decay.md)
- P3-A 投影控制面：[ADR-0005：Memory Embedding Projection Manifest 单一控制面](./0005-memory-embedding-projection-manifest.md)
- 实施计划：[个人数字分身公共能力演进计划](../plans/personal-twin-public-capability-evolution.md)

## 决策摘要

Plico 的公共产品面收敛为一个人的数字分身与外部大脑，不再把内核中所有实验性操作都等同于公共 API。

公共协议围绕少量稳定领域能力建立：对象底座、个人记忆、召回、会话、能力发现、只读运行状态；证据、claim、投影和冷热检索只在相应基础设施完成后加入。企业租户、集群控制、组织共享、任意 KG 写入、模型热切换和内核调试操作不属于个人数字分身的公共产品面。

当前约百个 `ApiRequest` variant 和通用 `ApiResponse` 不能继续作为稳定外部契约。切换公共协议时，不为旧 wire method 建兼容壳，也不让新旧公共协议双轨运行：TCP/UDS、`RemoteClient`、MCP 和 aicli 在同一变更中改用新的 typed public protocol；旧请求仅在仍有真实内核调用方时内部化，无调用方的 variant、DTO、handler、示例和失实说明直接删除。无真实消费者的 SSE/A2A adapter 已删除，不在新协议中预留位置。

## 为什么现在必须先收敛能力面

### 代码事实

| 当前事实 | 代码证据 | 能力边界 |
|---|---|---|
| CAS 使用 SHA-256 CID、读时校验并原子写盘 | `src/cas/object.rs`、`src/cas/storage.rs` | 可以承诺不可变对象与内容寻址；还不能自动称为带 provenance 的“证据” |
| Working Memory create/update/delete 均走唯一 persist-before-publish mutation，再进入有界异步 embedding 管道 | `src/memory/layered/mod.rs`、`src/kernel/ops/memory.rs`、`src/kernel/public_service.rs` | 13 项 typed endpoint 已能承诺 canonical acknowledgement；持久化失败返回 `DEPENDENCY_UNAVAILABLE` 且不发布 |
| 管道固定 buffer 1024、并发 4，失败指数退避，30 秒协调 Pending，重启后可恢复 | `AIKernel::start_workers`、`indexing_pipeline/reconciliation.rs` 及其测试 | 已有真实可靠性基础；队列深度、单 entry 错误和就绪时间尚未公开记录 |
| `MemoryEntry::embedding_state()` 从 entry 状态推导 `NotRequested/Pending/Ready` | `src/memory/layered/mod.rs` | 适合第一阶段 per-entry readiness；不能表达 Failed/Stale/AbsentByPolicy/温度 |
| Pending Working memory 可通过同 role/default namespace 的词法路径召回 | `LayeredMemory::recall_working_lexical`、`src/kernel/public_service.rs` | typed hit 保留 entry ID、分数、状态与 `matched_by=lexical_overlap`；未冒充 BM25/hybrid |
| memory update 生成新 entry 并 supersede 旧 entry，delete 为软删除 | `AIKernel::memory_update`、`memory_delete`、`public_service.rs` | typed 外部纠错/遗忘闭环已存在；late embedding 不得复活旧 revision |
| `plicod` 只解码 `PublicRequestHead`/`PublicRequest` 并直接调用 typed service | `src/bin/plicod.rs::handle_connection` | 旧 method 不能到达 legacy dispatch；已认证的未知 operation 才得到 `UNSUPPORTED_CAPABILITY` |
| `RemoteClient` 使用 `Result<PublicResponse, ClientError>` | `src/client.rs` | transport/frame/schema 与 typed domain failure 已分层 |
| TCP 从 envelope bearer 推导可信 role；UDS/Embedded/MCP 拒绝 payload auth | `src/bin/plicod.rs`、`src/client.rs`、`src/kernel/public_service.rs` | public operation input 不自报身份；个人 owner 与普通本地 role 边界明确 |
| MCP 与 aicli 使用精确 13 项点号 operation 并构造 `PublicCommand` | `src/bin/plico_mcp/tools.rs`、`src/bin/aicli/input.rs` | 无 generic passthrough、旧 action table或 runtime adapter；catalog parity 由测试约束 |
| 无真实消费者的 `plico-sse` binary、测试、配置与 cold MCP handler 已物理删除 | `src/bin/`、`src/config.rs`、`Cargo.toml` | 不再把同步响应或 URL helper 包装成 A2A/SSE 能力 |
| `HealthReport` 会调用 LLM 并写入、删除 CAS 探针对象 | `src/kernel/ops/dashboard.rs` | 不是轻量、只读 readiness，不适合健康探针 |
| tenant management、cluster/distributed、`EvictCold`、public group recall 已从 enum、handler、DTO 与测试中物理删除 | `src/api/semantic.rs`、`src/kernel/handlers/`、`src/kernel/ops/` | 产品边界清债真实完成；旧 wire enum、33 个 public `tenant_id` 和 Core verbs 仍使公共面过宽 |
| Object 域已有真实 `Bm25Index`，以 CAS CID 为键并与 vector 做 RRF；Memory 域没有 BM25 index | `src/fs/search/bm25.rs`、`src/fs/semantic_fs/mod.rs`、`src/memory/layered/mod.rs` | object search 可声明 BM25 projection；memory 只能声明以 entry ID 为键的同域 lexical overlap，不能借用 CID 分数冒充 BM25 |

当前切换证据：typed service、auth bootstrap、durable mutation、role-scoped session 定向测试
通过；`cargo check --lib/--bins`、`cargo clippy --lib -- -D warnings` 与
`EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib`（2183 passed）通过；同环境完整
`cargo test` 也通过（observability suite 中一个既有 ignored test 保持 ignored）。该数字只记录
本次 cutover 验收，不作为 README 中长期手工维护的质量计数。

### 结论

当前已经开发了真实能力，但它们被一个过宽、可选字段过多、认证不一致的协议遮蔽。正确演化方向不是继续给 `ApiRequest` 加 variant，而是建立稳定的领域服务边界，把真实能力组成可验证闭环。

### P0 实际状态快照（2026-08-13）

| 状态 | 项目 | 证据与后续 |
|---|---|---|
| 已完成 | 删除 tenant management request/DTO/handler/store、`CrossTenant` 权限与持久化调用 | `rg` 在 `src/**/*.rs` 中对 `CreateTenant/ListTenants/TenantShare/CrossTenant/TenantStore` 为零；不得恢复为个人 vault 的“可选模式” |
| 已完成 | 删除 cluster request/DTO、distributed stub、node ping 与 checkpoint migration ticket | `rg` 对 `ClusterJoin/ClusterLeave/ClusterStatus/NodePing/ClusterManager/MigrationTicket` 为零 |
| 已完成 | 删除 `EvictCold` request/DTO/handler/实现及公开 cold 叙述 | `EvictCold`/`cold_objects` 定义与 dispatch 为零；`evict_cold` 仅保留在拒绝旧 wire method 的 negative serde test。未来 thermal 必须建立 projection control plane 后重新立项 |
| 已完成 | 删除 `RecallVisible`、`DiscoveryScope::Group` 与 public `group:*` 接受路径 | public parser 只接受 private/shared；持久化 `MemoryScope::Group` 仍存在，不能据此宣称 group capability |
| 已完成 | 删除 `plico-sse`、对应测试、SSE port/Agent Card URL 死配置、MCP cold handler 和虚假说明 | 不保留未来 adapter 占位配置 |
| 已完成 | plicod、RemoteClient、MCP 与 aicli 一次切到 `plico.personal.v1` | public path 不导入/构造旧 `ApiRequest`/`ApiResponse`，不保留 wire adapter |
| 已完成 | side-effect-free readiness、capability catalog、typed memory entry/hit 与 13 项 direct service | schema、service、MCP tool、aicli parser 由 exact parity tests 约束 |
| 已完成 | TCP personal-owner credential 本地 bootstrap | 首次 persist-before-publish，重启复用，0600 单一 credential file，日志/返回不含 token；不新增业务 operation |
| 未完成 | 11 个 `Core*` variants/handler 仍是内部可调用旧命令 | 已从 plicod/client/MCP/aicli transport 隔离；下一步按内部调用图重写后物理删除，不建兼容壳 |
| 未完成 | 旧 semantic request 仍有内部 `tenant_id`，存储/permission/CAS/Memory/KG 也保留历史字段 | typed public schema 已不暴露；持久化字段只能通过离线迁移删除 |
| 已完成 | `src/mcp/tests.rs` 已从旧三工具 fixture 切为 exact 13-tool typed client | unknown old tool negative test、typed put/get/search 与 descriptor parity 均通过 |

### 一次性迁移风险

- `tenant_id` 已进入 `AIObjectMeta`、`MemoryEntry`、KG redb、checkpoint/passport 与权限上下文。新 public schema 必须先不暴露它；内部字段的改名或删除要用可备份、可计数、可校验的离线 migrator，禁止 serde alias 和运行时双读。
- 已删除的 `CrossTenant` variant 可能仍出现在旧 permission snapshot，`tenant_index.json` 也可能作为孤儿文件存在。preflight 必须显式识别并报告，不能因整份反序列化失败而静默丢弃其他个人权限。
- 历史 `MemoryScope::Group` 已写入 memory snapshot 和 checkpoint。迁移为 private、shared 或未来 credential-bound role scope 会改变可见性，必须单独 ADR 裁决，不能用默认分支静默吸收。
- 历史 `EvictCold` 可能已把 canonical object 放进 recycle bin。发布前应关联 audit/recycle state 做恢复审计；删除 handler 不等于数据已经恢复。
- cluster/Core 删除不需要持久化兼容；测试或旧 wire 调用方只能得到 `UNSUPPORTED_CAPABILITY`，不得翻译。

## 产品不变量

以下不变量高于具体 API 名称和当前实现：

1. **一个自然人拥有一个本地 personal vault**。`PLICO_ROOT` 对应个人控制的数据边界，不创建企业组织树或 SaaS tenant 控制面。
2. **canonical memory 和原始 evidence 是事实恢复基础**。向量、BM25、KG、摘要、Wiki、缓存和人类文档均是可重建 projection。
3. **写成功先表示 canonical commit**。projection 可以 Pending、Failed 或 Stale；其失败不得回滚已经成功持久化的个人记忆，除非 canonical 持久化自身失败。
4. **AI 直接消费 typed memory、evidence 与 recall result**。Markdown、表格、PPT、GUI 是按需生成的人类投影，不是 AI 必经的主数据格式。
5. **纠错是追加版本，不是静默覆盖**。删除、supersede、冲突和用户确认必须可解释；模型生成文本不能自证为 evidence。
6. **冷热只改变检索成本**。temperature 不改变事实真值、重要度、保留义务或 canonical 字节。
7. **agent 是同一数字分身内的执行角色**。角色不是企业用户或租户；外部请求中的自报字符串不能建立身份。
8. **所有公开能力必须可发现、可认证、可观察、可失败**。没有真实 handler、真实状态或验收测试的字段不得出现在 capabilities 中。
9. **公共结果必须可继续操作**。memory recall 至少返回 entry ID、内容、状态和检索依据，不能只返回失去身份的字符串。
10. **切换采用单一真值路径**。不为保留旧 wire method 建 adapter，不长期双写旧状态和新 manifest。

## 自然演化决策机制

用户提出的方向、论文中的机制和现有代码都只是设计输入，不自动成为产品承诺。每项候选能力按同一状态机推进：

```text
DesignInput -> ProductInvariantCheck -> ReproducibleExperiment
            -> AcceptedADR -> SinglePathImplementation -> PublicCapability
            \-> Rejected/Deleted
```

- `ProductInvariantCheck` 先检查个人 vault、canonical-first、可逆腐败、身份安全和可解释失败；与不变量冲突的方向直接拒绝，不以“用户曾提出”作为保留理由。
- `ReproducibleExperiment` 必须有固定 dataset/clock/provider、真实 ID、消融对照、延迟/资源成本和 fail-closed 报告。只有案例演示或测试专用 mock 不足以进入 ADR。
- `AcceptedADR` 明确领域归属、状态所有权、持久化迁移和删除项。没有 ADR 的研究实现不得出现在 capability catalog。
- `PublicCapability` 必须同时满足 typed schema、auth、handler、transport parity、observability 和 benchmark gate；实现存在但门禁不足时保持 internal/unsupported。
- 被证伪或无调用方的候选直接删除；不保留 deprecated marker、别名、兼容壳或双轨状态。

## 明确非目标

- 企业多租户、组织/团队知识库、组织级 RBAC、租户计费与跨租户共享；
- 分布式集群、HA 控制面、节点迁移或集群成员管理；
- 桌面文件系统、Office 编辑器或以文件树作为知识真值；
- 让 LLM 自动覆盖高影响个人事实、自动硬删除低热度知识；
- 把相似度、KG edge 或较新文本直接表述为现实因果或事实真值；
- 将所有内核调试、模型管理、hook、prompt 和 scheduler 操作公开给普通 AI 客户端；
- 为旧协议保留隐形兼容层或为未实现能力返回空成功。

## 统一领域模型

```text
PersonalVault（一个人的数据边界）
  ├── EvidenceObject (CID, immutable bytes, source metadata)
  ├── MemoryStream (stable memory identity)
  │     └── MemoryRevision* (append-only canonical revisions)
  │            ├── EvidenceLink*
  │            └── Claim* (explicit epistemic interpretation)
  ├── ProjectionManifest*
  │     └── ProjectionArtifact (embedding/BM25/KG/summary/wiki/document/...)
  ├── RecallExecution* (budget, layers, hits, coverage, feedback)
  ├── Session* (role-scoped cognitive continuity)
  └── AgentRole* (local cognitive/execution role, credential-bound)
```

### PersonalVault

`PersonalVault` 是单个自然人的数据所有权边界，部署上由个人控制的 root 与密钥材料确定。现有 `tenant_id` 只能在迁移期作为内部默认命名空间存在，不能进入新的 public v1 schema，也不能被解释为企业 tenant。

### EvidenceObject

Evidence 是不可变的观察材料，不等于已经确认的事实。

目标字段：

- `cid`：原始字节的内容地址；
- `content_type`、`byte_length`；
- `source_locator`、`source_kind`；
- `observed_at`、`ingested_at`；
- 可选外部签名/哈希；
- 摄取策略与解析器版本。

当前 CAS 已实现 CID、字节、基础 metadata 和完整性校验，但没有完整 source/time provenance。因此第一阶段对外名称是 `object.*`，不能提前把所有 CAS 对象称为 `evidence.*`。

### MemoryStream 与 MemoryRevision

目标模型将稳定记忆身份与具体 revision 分开：

- `memory_id`：跨纠错保持稳定；
- `revision_id`：一次不可变 revision；
- `supersedes_revision_id`；
- `content`、`memory_type`、`cognitive_tier`、`importance`；
- `created_at`、`valid_from/valid_to`；
- `evidence_links`、`epistemic_state`；
- `deleted_at` 或 retention event。

当前 `MemoryEntry::id` 实际是 revision 级 ID，`memory_update` 会创建新 ID。因此 public 第一阶段必须如实使用 `entry_id`，不得伪造尚不存在的 stable `memory_id`。稳定 identity 在 evidence/claim 迁移阶段一次性加入。

### Claim

Claim 是对 evidence 的可解释断言，而不是任意文本 memory 的别名。目标字段包括 subject/predicate/object 或受约束 payload、epistemic state、valid time、证据边和用户确认事件。

当前 contradiction、KG 和 ingest 代码只能作为候选生成/分析工具，不能据此宣称 claim 已实现。`claim.*` 在 provenance、稳定 memory identity 和 review event 完成前不进入 public capabilities。

### ProjectionManifest 与 ProjectionArtifact

Manifest 是已准入派生类型的唯一状态源，最终目标至少记录：

- canonical kind/id/revision/content hash；
- projection kind、schema/builder/model/dimension；
- `Queued/Building/Ready/Failed/Stale/AbsentByPolicy`；
- source watermark、attempt、稳定错误类别、updated_at；
- artifact locator；只有通过独立 DECAY/thermal ADR 接纳后，才增加正交的 retrieval temperature
  事件或 schema version。

当前 `embedding: Option<Vec<f32>>` 和推导的三态只支持第一阶段 embedding readiness。引入 manifest 时必须一次性重写调用点并删除 canonical entry 内嵌 embedding、`embedding=None` 状态推导和重复状态源。

P3-A 的冻结边界见 ADR-0005：初始唯一准入 kind 是 `memory_embedding`，不迁移 Object HNSW/BM25，
不预留 temperature 字段，也不因 artifact Ready 自动启用 Memory vector recall。V1-B canonical 已无
inline durable vector，因此一次性切换从 canonical revision 全量 Queued/rebuild，不能把 runtime
向量冒充 migration artifact。

### BM25 的领域归属与演化裁决

BM25 不是跨领域共享的“检索分数服务”，而是绑定特定 canonical ID 空间、tokenizer、字段和 watermark 的派生投影。

1. **Object 域：已实现。** 当前 `SemanticFS` 的 `Bm25Index` 以 CAS CID 为 key，create/update/delete/rebuild 与 object lifecycle 同步，可与 object vector hits 做 RRF。它可以在 `object.search` 中如实标注 `matched_by=bm25`，但仍是可删除、可重建的 object projection，不是 evidence 或 canonical metadata。
2. **Memory 域：未实现 BM25。** 当前 `LayeredMemory::recall_lexical` 直接扫描 Working/LongTerm entry，以 `MemoryEntry::id` 计算规范化词项重叠，承担 Pending embedding 的 read-after-write。它必须命名为 `lexical_overlap`，不能命名为 BM25，也不能复用以 CID 为 key 的 object BM25 score。
3. **是否新增 Memory BM25：待消融。** 候选方案必须使用 entry/revision ID、相同 active/deleted/superseded/role filter 和独立 projection watermark。只有在固定 LongMemEval/LoCoMo/CJK 与个人数据 fixture 上，相比 `lexical_overlap` 和 vector-only 显著改善 recall/latency/资源曲线，且不破坏写后立即召回，才提交新 ADR。否则保持现状并删除候选代码。
4. **Projection/Thermal：不预承诺 BM25。** 若未来接受 Memory BM25，它才进入 projection manifest，并可独立于 vector 选择 Hot/Warm/Cold/Dormant residency；temperature 只能移动或卸载 postings/artifact，不能改变 canonical revision。Dormant discovery 是否使用压缩词项目录、其他无模型索引或完全不同结构，由 DECAY 消融决定，不默认等于 BM25。
5. **公共协议只报告事实。** 下一 typed slice 的 `memory.recall` 只报告 `matched_by=lexical_overlap`；`bm25` 仅可出现在后续 `object.search`。Memory capability catalog 在实验与 manifest 完成前只把 `memory_bm25` 标为 `unsupported`；研究状态留在 ADR/实验记录，不混入 public capability 状态机，也不能表述为 degraded。

### ThermalState

`Hot/Warm/Cold/Dormant` 只属于 projection residency。它与 `MemoryTier`、`MemoryType`、importance、TTL、deleted/superseded 正交。旧 CAS `cold_objects`/`evict_cold` 已因删除 object visibility 而物理删除；这次删除不表示 thermal 已实现。

### RecallExecution

目标 recall 是有预算的逐层检索执行，不是返回字符串数组：

- query、mode (`fast/balanced/deep`) 与 token/latency budget；
- time/type/tag/source filters；
- 每层候选、延迟、读取字节和水位；
- typed hit：entry/memory/revision/evidence IDs、内容、score components、`matched_by`；
- coverage、degradations、projection watermark；
- 可选 rehydrate job；
- 只对最终使用并确认有用的 hit 记录 thermal feedback。

下一条 typed vertical slice 只支持 `lexical_overlap`，并独立报告 embedding `Pending/Ready/NotRequested`；hybrid 需要 typed score/degradation 证据后再加入。`fast/balanced/deep` 在 projection manifest 和 cold discovery 建成前为 unsupported。

### Session

Session 是某个 AgentRole 的短期认知连续性边界，包含 session ID、事件序列水位、checkpoint 和本轮 token/操作统计。Session 不拥有 canonical memory，不是企业用户会话，也不能成为唯一 provenance。

当前 start/end、checkpoint、delta 有真实实现，可在完成统一 auth 和 typed response 后公开；intent prefetch 与 warm-context placeholder 必须单独通过真实性审计，不能自动纳入稳定 session contract。

### AgentRole 与认证

`AgentRole` 表示同一数字分身内的本地执行职责，例如 owner、conversation、research、projection。它不代表不同自然人或企业账号。

认证放在请求 envelope，一次绑定整次请求，不在每个 operation variant 重复可选 `agent_token`：

```json
{
  "protocol": "plico.personal.v1",
  "request_id": "uuid",
  "auth": { "bearer": "..." },
  "operation": "memory.create",
  "input": { "content": "..." }
}
```

- credential 在服务端解析为 `role_id`；operation input 不接受自报可信 role；
- TCP 必须认证；UDS 可由 owner-only socket 映射固定 local-owner role，不能由 payload 自选保留主体；
- Embedded 模式由宿主显式注入 trusted local context；
- permission 是个人 vault 内 role capability，不引入 tenant/organization RBAC；
- reserved `kernel/system` 永不成为公共 role 名。

### Observability

公共 observability 只暴露个人客户端需要的可行动状态：request ID、canonical commit、per-entry projection state、job state、degradation、readiness 和水位。不默认暴露原始内容、token、内部路径或所有 kernel counters。

`liveness`/`readiness` 必须只读、有界、无 LLM 调用、无 CAS 探针写入。深度诊断属于显式 owner maintenance 操作。

## 公共协议形态

### Typed envelope

成功：

```json
{
  "protocol": "plico.personal.v1",
  "request_id": "uuid",
  "ok": true,
  "data": { "...typed operation response...": "..." }
}
```

失败：

```json
{
  "protocol": "plico.personal.v1",
  "request_id": "uuid",
  "ok": false,
  "error": {
    "code": "NOT_FOUND",
    "message": "memory entry was not found",
    "retryable": false,
    "details": { "category": "memory_entry" }
  }
}
```

稳定错误码：`INVALID_ARGUMENT`、`UNAUTHENTICATED`、`PERMISSION_DENIED`、`NOT_FOUND`、
`CONFLICT`、`LIMIT_EXCEEDED`、`BUSY`、`PROVIDER_UNAVAILABLE`、
`DEPENDENCY_UNAVAILABLE`、`UNSUPPORTED_CAPABILITY`、`INTERNAL`。canonical storage、memory
persister 或 session durable I/O 不可用时使用 `DEPENDENCY_UNAVAILABLE` 且 `retryable=true`；
`details` 只放稳定类别，不透传 provider/storage 原错、内容或宿主路径。

Pending 不是错误。canonical 写入成功而 embedding 未完成时返回 success + projection state。Transport 连接失败、超时、invalid frame 和 invalid JSON 是 `ClientError`，不得压成 domain `ApiResponse`。

### 下一条 typed vertical slice：V1-A Personal Twin Truth Loop

V1-A 是 `plico.personal.v1` 的首次且唯一 public wire cutover，不是把旧协议包一层。调用图复核表明，先发布仅含 memory read/create 的六项协议会切断现有真实 object 摄取、个人纠错/遗忘和 session continuity，因此首次切换冻结为以下 13 项：

| Operation | 精确输入 | 精确输出 | 明确不支持 |
|---|---|---|---|
| `capabilities.describe` | 空对象 | protocol、精确 operation 集合、limits、consistency、unsupported | Rust enum 反射、内部 counters |
| `runtime.readiness` | 空对象 | `ready`、canonical store 状态、index worker 状态、embedding provider degradation | LLM/CAS 写探针、内部路径、token |
| `object.put` | UTF-8/base64 `content`、`tags` | CID、canonical commit | path、agent/namespace 自报、同步 projection 成功承诺 |
| `object.get` | CID | verified bytes、metadata | current-version/path 语义 |
| `object.search` | `query`、tag filters、`limit` | typed hits、实际 retrieval paths、provider degradation | memory BM25、静默 fallback |
| `memory.create` | UTF-8 `content`、`tags`；role 从 auth context 得到 | active Working entry view、canonical commit、embedding 三态 | tier、scope、importance、agent/namespace 自报、LongTerm |
| `memory.get` | `entry_id` | 同 role 的 active Working entry view | history、跨 role、deleted/superseded 包含开关 |
| `memory.recall` | `query`、`limit` | typed hits：entry view、真实 overlap score、`matched_by=lexical_overlap` | BM25、vector/hybrid、fast/balanced/deep、字符串数组 |
| `memory.index_status` | `entry_id` | `NotRequested/Pending/Ready` | ready_at、queue position、last_error、temperature |
| `memory.update` | `entry_id`、UTF-8 `content` | previous/new entry ID、新 active view、canonical commit | 原地覆盖、LongTerm 更新 |
| `memory.delete` | `entry_id` | tombstone time、canonical commit | 物理擦除、删除 history |
| `session.start` | 可选 `last_seen_seq` | session ID、真实 current watermark、changes | intent/prefetch/warm placeholder/假 checkpoint |
| `session.end` | session ID | session ID、真实 event watermark | auto checkpoint/consolidation/伪 last_seq |

13 项不是对内部能力的无条件背书。首次 wire cutover 所需四个真实性 blocker 已按单一路径
完成：Working create/update/delete 可失败且 durable-first；session 去除 placeholder 并按 role
过滤真实 event watermark/change；object search 返回实际 BM25/vector/tag/KG/reranker execution 与
typed degradation；readiness 只读且不探测 provider。后续能力仍须按自然演化机制逐项取证，
不得因为已经有 13 项协议就把内部实验面直接外放。

V1-A schema 使用 `deny_unknown_fields` 等价规则，防止被忽略字段制造虚假语义。`request_id`、`entry_id` 和 `session_id` 是非 nil UUID；memory text 为 1..=262144 UTF-8 bytes，object 解码后为 1..=10485760 bytes，tags 最多 32 项且每项为 1..=64 UTF-8 bytes，`query` 为 1..=8192 UTF-8 bytes，`limit` 默认 20、范围 1..=100。TCP bearer 最大 4096 bytes；UDS/Embedded 请求不接受 payload 自报 role，由 transport/宿主注入 context。任何越界、未知字段或错误 ID 返回 `INVALID_ARGUMENT`，不得截断或忽略。

### Superseded：已由 ADR-0005 切换为 `plico.personal.v2`

本节记录 P3-A 切换决策；当前生产契约已按 ADR-0005 破坏式切到 `plico.personal.v2`。上面的 13 项
`plico.personal.v1` 仅是历史协议证据，不再有 reader、alias、adapter 或 runtime fallback。

`NotRequested/Pending/Ready` 无法诚实表达 durable `Building/Failed/Stale/AbsentByPolicy`，因此不在
personal.v1 中扩展 enum 或增加兼容映射。P3-A 完成时一次切到 `plico.personal.v2`：原 13 项中的
12 项保持名字，删除 `memory.index_status`，新增 `projection.status` 与 owner-only
`projection.rebuild`，catalog 精确为 14 项；MemoryEntryView 同时删除 `embedding_state`。所有
transport、客户端、MCP、CLI、plico-agents 与 benchmark 同版本切换，v1 reader/alias/adapter 物理
删除。

`projection.status` 先按 canonical 当前 policy 鉴权，再区分 `observed/unreconciled/unavailable`；
只有 observed 才能返回 `queued/building/ready/failed/stale/absent_by_policy`。完整 content hash 可以
作为鉴权后 typed response 的 revision identity 证据，但禁止进入 trace、日志、metric label、错误、
未鉴权诊断或 capability catalog。`projection.rebuild` 的成功只承诺 durable Queued transition，
不承诺 artifact 已构建。Memory vector/hybrid recall 继续 unsupported，Ready artifact 不改变
`memory.recall` 的 `lexical_overlap` 行为。

### Phase 1 完整目标闭环

公共 v1 首次切换只包含以下 operation：

| Operation | 输入核心字段 | 输出核心字段 | 一致性 |
|---|---|---|---|
| `capabilities.describe` | 无 | protocol、operations、limits、consistency、unsupported | 只读静态+运行能力 |
| `runtime.readiness` | 无 | ready、workers、provider degradation | 只读、有界、无探针写 |
| `object.put` | bytes/encoding、tags | cid + commit state | CAS commit；projection 失败不得伪装 canonical 失败 |
| `object.get` | cid | bytes/encoding、metadata | 强读；CID 完整性校验 |
| `object.search` | query、filters、limit | typed object hits | BM25 可用；vector 可降级 |
| `memory.create` | content、tags | entry view + embedding state | canonical durable ack；projection eventual |
| `memory.get` | entry_id | typed entry view | canonical read |
| `memory.update` | entry_id、new content | old/new entry IDs + new state | append-only revision；projection eventual |
| `memory.delete` | entry_id | deleted_at | canonical soft delete |
| `memory.recall` | query、limit | typed hits + matched_by | lexical read-after-write；hybrid 需后续证据 |
| `memory.index_status` | entry_id | `NotRequested/Pending/Ready` | 当前 entry 的真实派生状态 |
| `session.start` | last_seen_seq | session_id、changes、水位 | role-scoped |
| `session.end` | session_id | last_seq | role-scoped |

第一阶段 `memory.create` 只承诺当前真正异步化且持久化语义清晰的 Working Memory。LongTerm 现有写入仍同步 embedding、执行语义去重和可选事实抽取；在架构裁决并统一 canonical-first 语义前，不将它混入同一个 public create。

第一阶段 typed entry 如实使用现有字段：

```json
{
  "entry_id": "uuid",
  "content": "...",
  "tags": [],
  "created_at": 0,
  "embedding_state": "pending"
}
```

不得加入不存在的 `memory_id`、evidence links、temperature、ready_at、queue position 或 failure detail。

### 能力发现

`capabilities.describe` 是公共产品表，不是对 Rust enum 的反射。每项能力至少包含：

- operation 名、stability、auth requirement；
- canonical/derived consistency；
- 支持的 content/retrieval mode 和限制；
- 当前 provider degradation；
- 明确的 unsupported 列表。

MCP tools、SDK 和文档必须从同一静态 capability catalog 生成或测试交叉一致，不能各写一份字符串说明。已删除的 Agent Card 不属于当前 transport 或能力发现面。

V1-A 必须显式报告 unsupported：`memory.vector/hybrid/bm25`、`object.update/delete/history`、session checkpoint/prefetch、`evidence.provenance`、`claim.*`、`projection.generate`、`projection.manifest`、`recall.fast/balanced/deep`、`thermal.*`、`dormant.rehydrate`、document/table/slides projection。unsupported 是事实状态，不是这些方向已获承诺。

## 外部能力面分类

### 基于现有真实能力、完成 Phase 1 切换后可公开

- CAS object put/get 和完整性校验；
- SemanticFS object search，明确 BM25/vector degradation；
- Working Memory canonical create/get/update/delete；
- Working Memory 词法 read-after-write；hybrid recall 只有在 typed score/degradation gate 完成后才可公开；
- per-entry `NotRequested/Pending/Ready`；
- 有界异步 embedding、重试、协调与重启恢复；
- session start/end 的基础 continuity、真实 event watermark 和 delta；
- non-mutating readiness 与 capability discovery；
- local UDS、authenticated TCP 和 MCP adapter。

### 只保留为内部 owner/maintenance 能力

- 直接 KG node/edge 写入、graph traversal 和所谓 causal path；
- model hot-swap、prompt override、hook register、cache invalidate；
- raw event/trace、permission mutation、agent resource 和 scheduler control；
- agent message/delegation/task tree、skill forge；
- host-path file import；
- memory move/promotion/compact/evict maintenance；
- detailed storage/queue/internal performance counters。

这些能力可以有真实内部调用方，但不应增加普通个人 AI 客户端的协议复杂度。后续若形成明确个人场景，再通过领域 API 对外，不直接暴露内核结构。

### 等基础设施完成后再公开

- `evidence.ingest/get`：等待 source locator、observed/ingested time 和 provenance；
- `claim.propose/review/correct/history`：等待稳定 memory identity、证据边和 review event；
- projection manifest、watermark、rebuild 与 model migration；
- Hot/Warm/Cold/Dormant、deep recall 和 rehydrate job；
- document/table/slides/GUI projection 与显式 absorb；
- 跨 projection 的 freshness/coverage 承诺。

### 公共面清债状态

- 已删除：tenant management request/DTO/handler/store、cluster/distributed、`EvictCold`、public group recall、MCP 虚假说明、`plico-sse`；
- 待删除：互相重复的 `Core*`、`Store/Get/List/Patch/Remove/Control/Inspect/Invoke/Ask` 多套 verbs；aicli 必须先在同一 cutover 改用 typed operation；
- 待从 public schema 删除：33 个 `tenant_id`、payload `agent_id/token` 和所有内部管理/调试 operation；
- 已删除：SSE port/Agent Card URL 死配置与旧 client 文档；
- 通用 response 中无生产写入方的 optional 字段及配套 DTO；
- v53 旧里程碑中已被真实实现否定的 CognitiveTask 方案、硬编码性能/竞品数字和未完成 checkbox。

删除必须基于调用图和 transport E2E 测试逐项执行；确认死代码后直接删除，不加 deprecated marker、alias 或 compatibility handler。

## 关键架构冲突与裁决

### “Everything is an API”不等于“所有内核方法都是公共 API”

AI 需要语义 API，但语义 API 应表达个人记忆任务，而不是暴露所有实现旋钮。约百个 method + universal response 会让 capability discovery、认证、错误处理和测试组合爆炸。裁决：公共面按领域收敛，内部保持可组合工具。

### canonical-first 与 long-term 同步语义去重冲突

现有 long-term write 在 commit 前同步 embedding，并可能把新写入静默合并到相似 entry。异步 canonical-first 后，写时无法保持完全相同的 semantic dedup 行为。禁止用兼容壳隐藏该差异。

ADR-0005 已冻结裁决：所有 durable 写入始终创建 canonical revision；P3-A 同一切换中重写
LongTerm 调用点并物理删除同步写时 embedding/dedup。dedup/merge 只有在 P2-B 建成带证据和用户
确认的 reconciliation proposal 后才能恢复；在此之前明确 unsupported，不保留兼容语义。

### `embedding=None` 与 Dormant 冲突

今天 `None` 表示可索引 entry 的 Pending；未来 Dormant 也可能有意没有 embedding。若直接增加 thermal 字段，协调器会错误重建 Dormant。裁决：thermal 实施前先建立唯一 projection manifest，并一次性删除三态推导和内嵌向量状态源。

### CAS cold eviction 与可逆腐败冲突

旧 `evict_cold` 根据访问日志删除 SemanticFS 中的 CAS object，会把检索成本变化变成事实数据删除，现已移除。未来只允许卸载 manifest 指向的可重建 projection artifact；不得以相同名称恢复旧实现。

### `agent_id` 便利性与身份安全冲突

payload 自报 agent name 很方便，但不能证明调用者是谁。把 token 继续复制到少数 variants 会产生绕过。裁决：统一 auth envelope 在 dispatch 前生成 `RequestContext`，operation input 不决定可信 role。

### 本地优先与远程便利冲突

个人产品优先 Embedded/UDS；TCP 是显式远程能力。裁决：默认 loopback，UDS owner-only；TCP 必须 required auth。不得因“单用户”而省略远程认证。

新安装没有旧 registration wire 可用于 bootstrap。裁决：这不是第 14 个业务 operation；
`plicod` 在启动 worker、写 PID 和绑定 TCP 前，通过唯一 `AgentKeyStore` 首次创建固定
`personal-owner` credential，并先原子写入 `PLICO_ROOT` 下 owner-only（Unix 0600）的
`agent_tokens.json` 再发布到内存。重启复用同一值；失败则 daemon fail closed。stdout 只提示
固定文件名，tracing 只记录 created/existing 类别，绝不记录 token 或宿主私有路径。后续轮换
必须原子替换同一 map entry 和同一 credential file，不增加第二来源、runtime alias 或双读。

### 健康检查与自检深度冲突

健康探针调用 LLM 和写 CAS 会造成延迟、费用和数据变化。裁决：readiness 只读；深度 roundtrip/模型探针是显式 owner diagnostic job，不进入负载均衡健康端点。

### 文档投影与用户修改冲突

用户编辑生成的文档可能包含重要纠错，但文件保存不能静默覆盖 canonical。裁决：投影默认只读；`projection.absorb` 后续把修改作为新 evidence，经 diff/review 生成 memory revision。

## 公共切换与兼容策略

1. 新协议使用明确 protocol identity `plico.personal.v1`，不继续沿用当前仅在 `Create` 上出现、也未统一执行的 `ApiVersion 26.0.0` 叙述。
2. `plicod` 只反序列化 public request；包含合法 protocol/request ID 且已完成 TCP 认证的未知
   operation 返回 `UNSUPPORTED_CAPABILITY`，不翻译到新 operation。完全旧格式因无合法
   request ID 属于 invalid frame/schema，transport 关闭或返回协议错误，不伪造 typed response。
3. `RemoteClient`、MCP 和 aicli 与服务端同提交切换；不保留旧 client adapter，也不恢复已删除的 HTTP/SSE adapter。
4. 内部若仍需旧 enum，重命名为 crate-private internal command 并禁止 serde transport；随调用方重写逐步删除，但 public path 不经它转发。
5. 持久化格式变化使用一次性、可备份、可校验的离线 migrator；不在运行时长期双读双写。
6. 每次迁移若改变语义（尤其 long-term dedup、stable memory identity、projection manifest），先记录 ADR 裁决，再执行单路径切换。

## 第一阶段文件级落地建议

| 文件 | 变更 |
|---|---|
| `src/api/public/`（新） | 唯一 public request envelope、V1-A 13 个 typed operations/responses、error code、capability catalog；不得导入旧 `ApiResponse` |
| `src/api/semantic.rs` | 改为内部 command 后按调用图削减；删除 public serde 职责和无调用 DTO，而非给新协议写适配层 |
| `src/kernel/public_service.rs`（新） | 以 authenticated `RequestContext` 直接调用 Working Memory primitive；不构造旧 `ApiRequest`，不通过旧 wire dispatch 转发 |
| `src/kernel/ops/memory.rs` | 提供唯一 typed Working create/read/update/delete/recall/status 路径；update/delete 必须先 durable commit 后 publish |
| `src/memory/layered/mod.rs` | 增加 owner-scoped entry lookup/view；不增加假的 stable memory ID 或 thermal 字段 |
| `src/kernel/ops/indexing_pipeline.rs` | 只提供真实 stats；per-entry 状态从 entry 查询。若要 queue depth/last error，先真实记录再暴露 |
| `src/client.rs` | `KernelClient` 改 typed public request；transport 返回 `Result<_, ClientError>`，不压成 domain error |
| `src/bin/plicod.rs` | 仅接受 public protocol；统一 frame/JSON/domain error；在 decode 后、dispatch 前完成 auth |
| `src/bin/plico_mcp/*` | tools 与 13 项 capability catalog 精确相等；已删除的 cold/边界外说明不得恢复 |
| `src/bin/aicli/*` | 与公共 client 使用同一 typed request，不直接绕过 kernel 方法形成另一套语义 |
| `src/config.rs`、`src/client.rs` | SSE port/Agent Card URL 和 SSE consumer 说明已删除；不得恢复未来占位配置 |
| `src/kernel/ops/dashboard.rs` | 拆分只读 readiness 与显式 deep diagnostic，readiness 禁止 LLM/CAS mutation |
| `src/kernel/ops/fs.rs` | cold eviction 已删除；后续 projection store 建成后另立 thermal maintenance，不复用 object access log 语义 |
| `src/api/INDEX.md`、`src/bin/INDEX.md` | 记录新 public contract、非目标和 transport 一致性要求 |
| `tests/public_protocol_*` | UDS/TCP/MCP contract、auth matrix、typed errors、capability parity、memory eventual consistency |

## 验收原则

- 每个 capabilities operation 都有真实 handler、typed success、typed failure 和至少一个 transport E2E；
- 每个 public request 建立以 `request_id` 和 `operation` 为根的结构化 span；鉴权、canonical validate/persist/publish、projection enqueue、retrieval path、session transition 记录阶段、对象 ID、候选/命中数、耗时与错误类别，使数据流和逻辑流可重建；
- tracing 不记录 bearer、memory/object 正文、完整 query、provider 原始错误或宿主私有路径；测试聚焦关键不变量与代表性故障注入，不用穷举输入替代可观测性；
- 未列入 capabilities 的内部 operation 无法通过 TCP/UDS/MCP 调用；
- Required auth 覆盖公共 operation 100%，reserved identity 无法自报；
- Working memory canonical ack 不依赖 embedding provider 成功；
- 同请求后 lexical recall 可见，embedding 最终 Ready，provider failure 保持 Pending/可重试；
- queue saturation、daemon restart、update/delete late task 均不丢 canonical、不复活旧 revision；
- `memory.index_status` 不返回编造的 queue position、ready time 或 error；
- transport timeout 与 domain error 在 SDK 类型上可区分；
- readiness 无写盘探针、无 LLM/embedding 网络调用；
- MCP schema、capability catalog 与实际 handler 集合完全相等；
- public schema 不含 tenant/cluster/group/team/cold-evict 语义；
- thermal、claim、projection 在实现前稳定返回 `UNSUPPORTED_CAPABILITY`，benchmark 也报告 unsupported 而不是 pass。

## 后果

正面后果：外部 AI 获得更小、更稳定且可自解释的能力面；真实异步记忆能力能够被安全使用和验证；未来 evidence/claim/projection/thermal 可以按领域自然加入，而不是继续堆 variant。

代价：这是一次 public protocol major cut，需要同时修改所有本仓库客户端；一些现有实验功能会从公共面消失；long-term canonical-first 语义和 persistent projection manifest 需要后续架构迁移。

这些代价优于继续维护一个无法完整认证、无法准确发现、无法为每个 optional response 字段提供真实性保证的外部协议。
