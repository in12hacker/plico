# 个人数字分身公共能力演进计划

- 状态：Active（P0、P1、V1-A 与 V1-B 已完成；P3-A Rust 单轨切换已落地，B2 消费者与发布证据门禁进行中）
- 日期：2026-08-13
- 架构依据：[ADR-0003](../adr/0003-personal-twin-domain-and-public-capability-contract.md)
- 产品依据：[ADR-0001](../adr/0001-personal-digital-twin.md)
- 记忆与腐败依据：[ADR-0002](../adr/0002-canonical-memory-and-reversible-retrieval-decay.md)
- Canonical revision 依据：[ADR-0004](../adr/0004-canonical-revision-ledger.md)
- P3-A Projection Manifest 依据：[ADR-0005](../adr/0005-memory-embedding-projection-manifest.md)

## 目标

把已经真实开发并通过测试的能力组成可供外部 AI 稳定使用的个人数字分身 API；同时为 evidence、claim、projection 和 thermal recall 建立自然演化路径。计划不扩展企业多租户，不把旧 `ApiRequest` 全量包装进新 envelope，也不为未实现能力造字段或 mock。

## 工作原则

1. 每一阶段形成可独立验收的端到端用户闭环，不按文件数量宣布完成。
2. 公共 capability 只有 `supported` 或 `unsupported`，没有“空数组即成功”的第三种状态。
3. transport、auth、domain、projection failure 分层建模。
4. canonical 先于 projection；投影失败不谎报 canonical 失败，也不谎报 ready。
5. 语义替换采用单路径 cutover；旧代码确认无调用后直接删除。
6. 先使用本地 Embedded/UDS dogfood，再开放 authenticated TCP/MCP；已删除的 HTTP/SSE 不预留占位。
7. 用户方向、论文机制和现有实现都是 `DesignInput`，只有通过产品不变量、消融实验、ADR 和端到端门禁后才成为 public capability；未通过则保持 research/unsupported 或删除。
8. “已实现”与“已公开”分开记账；测试 mock、空成功、单案例和无调用方代码不能升级状态。

## 阶段总览

| 阶段 | 用户可获得的新增闭环 | 前置依赖 | 退出门禁 |
|---|---|---|---|
| P0 公共面冻结与真实性清债（Completed） | 明确哪些能力支持/不支持 | 当前代码调用图 | 旧假能力无法远程调用，文档与 handler 一致 |
| P1 / V1-A 个人记忆公共闭环（Completed） | 13 项 typed capability；create/get/update/delete/recall/index status | Working Memory + async pipeline | canonical-first、read-after-write、最终索引、全 transport/auth E2E |
| V1-B Canonical Truth Firewall（Completed） | stable `memory_id`、append-only revision ledger、tombstone | 已完成的 V1-A truth loop | 旧 revision 不变、迁移 fail closed、唯一 writer、无兼容壳 |
| P3-A Embedding Projection Control Plane（Active） | `memory_embedding` 唯一 manifest、重建、水位；不激活 vector recall | V1-B stable revision identity/hash | personal.v2 exact-14、全部消费者和发布证据同轨；无 v1/inline 双真值 |
| P2-B Evidence 与 Claim | 来源可追溯的摄取、纠错、历史 | V1-B stable identity；可与 P3-A 设计并行但不阻塞其实施 | provenance 完整、版本可恢复、冲突不静默覆盖 |
| P4 候选：可逆 Thermal Recall | Hot/Warm/Cold/Dormant + deep | P3 manifest + DECAY 接纳决策 | DECAY gates、canonical hash 不变、deep 可发现；失败则不实施或降为更简单层级 |
| P5 人类侧按需投影 | 文档/表格/PPT/图形导出与 absorb | P2-B/P3-A provenance | 投影可删可重建，编辑不静默覆盖 canonical |

## P0：公共能力面冻结与真实性清债（Historical / Completed）

### 当前执行账本（2026-08-13 共享工作树）

| 状态 | 工作项 | 剩余风险 |
|---|---|---|
| 已完成 | tenant management、`CrossTenant`、cluster/distributed/node ping 物理删除 | 旧 permission snapshot 与孤儿 `tenant_index.json` 仍需 migration preflight |
| 已完成 | `EvictCold`、cold DTO/handler/旧行为测试物理删除；仅保留拒绝 `evict_cold` wire method 的 negative schema test | 历史 recycle bin 中可能有被自动软删除的 canonical object |
| 已完成 | public `RecallVisible`、`DiscoveryScope::Group` 与 `group:*` 接受路径删除 | 持久化 `MemoryScope::Group` 尚未迁移 |
| 已完成 | `plico-sse`、SSE 测试/配置、Agent Card URL helper、MCP cold handler 与虚假能力说明删除 | 不保留未来 adapter 占位配置 |
| 已完成 | TCP/UDS/`RemoteClient` 已只走 `plico.personal.v1`；旧 `ApiRequest`/`ApiResponse` 不再是 public wire | 旧内部命令仍按真实调用图继续清债，不得重新接回 transport |
| 已完成 | Core/poly public verbs、aicli legacy 双调用与 generic MCP passthrough 已删除 | 后续内部无生产调用方代码仍直接物理删除 |
| 已完成 | 新 public schema 不含 `tenant_id`、自报 agent/role 或企业 scope | 底层 legacy namespace 只允许通过离线迁移裁决 |
| 已完成 | capability catalog、只读 readiness、typed memory DTO/error 与 transport parity | catalog 精确保持 13 项；未实现能力继续 unsupported |

### 架构组

- 持续维护 ADR-0003 的事实状态；尤其裁决 long-term 写时 semantic dedup 是否改为异步 reconciliation proposal。
- 定义 public v1 capability catalog、typed errors、auth `RequestContext` 和 transport failure。
- 明确 `tenant_id` 仅内部 default namespace，public schema 不出现 tenant。
- 对每个新方向执行 `DesignInput -> invariant -> experiment -> ADR -> implementation -> capability`，不得因用户提议或论文存在直接进入 roadmap 承诺。

### 开发组

- 新建 typed public protocol 和 public service，不能把旧 `ApiRequest` 转成新 request 再转回。
- `plicod`、`RemoteClient`、MCP、aicli 同提交切换公共协议。
- 将 auth 从少数 variant 字段提升到 envelope；服务端解析 credential 后构造 role context。
- MCP tool schema 从 capability catalog 生成或受 parity test 约束。
- 拆出只读 readiness；现有 CAS roundtrip/LLM health 变成显式 owner diagnostic。
- 保持已删除的边界外 variants 不回流；完成重复 core verbs、管理与调试 operation 的删除/内部化。
- 删除残留 SSE 配置/说明和所有 public `tenant_id`；底层存储迁移不与 public cutover 双轨绑定。

### 测试组

- schema snapshot：public operation 精确集合；
- capability-to-handler 与 MCP-to-capability 集合相等；
- TCP/UDS/MCP 对未知和旧 operation 一致返回 `UNSUPPORTED_CAPABILITY`；
- public auth matrix 参数化覆盖 100% operations；
- transport disconnect/timeout/invalid frame 不成为 domain error；
- readiness 零 CAS 写、零 LLM/embedding 请求。

### 可观测性门禁

- public service 为每个请求建立 `request_id/operation/transport/role` 结构化 span；role 只能来自可信 context；
- memory mutation 记录 `entry_id`、revision transition 与 `validate -> persist -> publish -> enqueue` 阶段；object search 记录 query byte 数、filters、实际 path 的 candidates/accepted 与 degradation 类别；session 记录 session ID、last_seen/current watermark 和状态转换；
- 日志禁止 bearer、正文、完整 query、provider 原始错误及宿主私有路径；调试输出用于还原数据流和逻辑流，不成为成功判定的唯一来源；
- 测试使用代表性正常、持久化失败、provider degradation、restart、late task、auth failure 场景验证不变量，不构造穷举组合掩盖设计缺陷。

### 审计组

- `rg` + 编译调用图确认删除项无真实调用方；
- 检查所有 public response 字段至少一个真实生产写入方；
- 检查 token、内部路径、原始内容不进入 readiness/log 默认输出；
- 检查 UDS 权限、TCP 默认 loopback、远程 required auth 和 CORS。

### P0 gate

- `cargo test --lib`、全量 `cargo test`、`cargo clippy -- -D warnings`；
- `cargo build --bin plicod --bin plico-mcp --bin aicli`；不得重新增加无消费者 adapter；
- public operation 数量有显式 snapshot，不能随内部 enum 增长；
- 不存在 public tenant/cluster/group/team/cold-evict 字段；
- 所有删除均为物理删除，无 deprecated marker/alias/adapter。

## 历史完成切片：V1-A Personal Twin Truth Loop

V1-A 已完成首次 `plico.personal.v1` wire cutover：TCP、UDS、`RemoteClient`、MCP 和 aicli 一次切换，旧 `ApiRequest` 不再作为 public protocol。以下边界保留为回归契约，不因后续 V1-B 内部持久化迁移而放宽。

### 精确协议边界

TCP Request envelope 固定为：

```json
{
  "protocol": "plico.personal.v1",
  "request_id": "uuid",
  "auth": { "bearer": "transport credential" },
  "operation": "memory.create",
  "input": { "content": "...", "tags": [] }
}
```

- TCP 必须有 bearer；owner-only UDS 由 peer/socket policy 注入 local-owner context，payload 不得自报 role；Embedded 由宿主注入 context。
- schema 采用 `deny_unknown_fields` 等价规则；`request_id`/`entry_id` 必须是 UUID。TCP 的 `auth.bearer` 必填且非空；UDS/Embedded payload 不接受 bearer/role，context 由 transport/宿主注入。
- `operation` 精确允许 13 项：`capabilities.describe`、`runtime.readiness`、`object.put/get/search`、`memory.create/get/recall/index_status/update/delete`、`session.start/end`。
- `memory.create` 只接受 1..=262144 UTF-8 bytes 的 text 与最多 32 项 tags（每项 1..=64 UTF-8 bytes），固定创建 private Working entry；不接受 agent、namespace、tier、scope、importance、TTL 或 stable `memory_id`。
- `memory.get/index_status` 只读取 authenticated role 的 active Working entry；deleted/superseded/history 视图暂不进入本切片。
- `memory.recall` 只接受 1..=8192 UTF-8 bytes 的 query 与 limit（默认 20、范围 1..=100），只调用同域 `lexical_overlap`；hit 返回 `entry_id/content/tags/created_at/embedding_state/score/matched_by`，其中 `matched_by` 固定为 `lexical_overlap`。
- 越界、未知字段、错误 UUID 返回 `INVALID_ARGUMENT`，不截断、不忽略；未列出的 operation 返回 `UNSUPPORTED_CAPABILITY`。
- `object.search` 必须返回实际执行的 BM25/vector/tag 路径及 embedding degradation；`session.start/end` 只包含真实 changes/watermark，不含 warm placeholder、prefetch、checkpoint 或 consolidation；memory BM25、memory vector/hybrid、thermal 与所有旧 method 均返回 `UNSUPPORTED_CAPABILITY`。
- 首次 wire cutover 前必须一起完成 memory durable mutation、session truth、object diagnostics 和 side-effect-free readiness。任一项未通过则暂停切换，不发布六项降级协议。

### 文件级单路径 cutover 门槛

| 文件/区域 | 合入门槛 |
|---|---|
| `src/api/public/`、`src/api/mod.rs` | 定义唯一 envelope、13 个 input/output、typed error、静态 capability catalog；代码不得导入旧 `ApiResponse` |
| `src/kernel/public_service.rs` | 直接调用 Working Memory、persister、indexing pipeline 与只读 readiness primitive；`rg 'ApiRequest|handle_api_request'` 在该文件为零 |
| `src/kernel/ops/memory.rs` | `remember_working_scoped_with_id` 提供 crate 内 typed primitive；get/recall/status 统一 active/role filter；不新增兼容 wrapper |
| `src/client.rs` | `KernelClient` 接受 public request 并返回 `Result<PublicResponse, ClientError>`；连接、timeout、frame/JSON 错误不能变成 domain response |
| `src/bin/plicod.rs` | 唯一反序列化类型是 public envelope；旧 method/未知 operation 返回 typed `UNSUPPORTED_CAPABILITY`，不翻译旧 enum |
| `src/bin/plico_mcp/*` | 13 个独立 tool 与 capability catalog 精确相等；无 generic passthrough、无旧 action table、无自报 agent |
| `src/bin/aicli/*` | 仅经同一 `KernelClient` 发 13 个精确 operation；删除 Core/poly verbs 和直接 kernel 绕行，不保留隐藏 legacy mode |
| `src/api/semantic.rs`、`src/kernel/api_dispatch.rs`、`src/kernel/handlers/core_ops.rs` | 从 serde/public transport 脱离；Core variants、handler、测试物理删除。仍需的内部 command 改为 crate-private 且 public path 不引用 |
| `src/config.rs` 与索引文档 | SSE port/Agent Card URL 残留已删除；公共 transport 列表只含真实存在且通过测试的 Embedded/UDS/TCP/MCP |
| `tests/public_protocol_*` | schema snapshot=13 项；旧 method negative tests；auth matrix；TCP/UDS/MCP payload parity；object/memory/session truth、restart/read-after-write/status gate |

### V1-A 完成定义

- create 成功只表示 Working canonical 已持久化；embedding provider 失败仍返回成功加 `Pending`，重启后可协调；
- create 后立即 lexical recall 命中率为 100%，返回同一 `entry_id`，不使用 substring proxy；
- capability catalog、MCP tools、server handlers 和 schema snapshot 集合完全相等；
- `rg` 在 public schema/transport 中对 `tenant/group/team/cluster/cold/Core` 为零；
- 旧 protocol 没有 adapter、alias 或双轨监听；
- 全量 Rust test、clippy、三个保留 binary build 与真实 transport E2E 同时通过。

### 完成事实（2026-08-13）

- capability catalog、typed public schema、MCP tools 与 daemon handler 精确保持 13 项；未知及旧 operation fail closed；
- TCP bearer、owner-only UDS、Embedded host context 与 MCP local owner 均不接受 operation payload 自报身份；
- Working Memory create/update/delete 已是 persist-before-publish；embedding enqueue 失败由 Pending reconciliation 接管；
- object search 返回真实 BM25/vector/tag/KG/reranker execution 与稳定 degradation，readiness 保持只读；
- aicli 与 plico-agents dogfood 已使用同一 typed contract，真实 UDS 13-operation gate 和真实 LLM evidence loop 已通过；
- thermal、deep、memory BM25、evidence、claim、history、hard erase 和人类投影没有因此升级为 supported。

## P1：个人记忆公共闭环（Historical / Completed）

### 公共 API

P1 以一个 13-operation 单路径 vertical slice 保持现有个人工作流完整：能力/readiness、object put/get/search、Working Memory create/get/recall/index_status/update/delete、session start/end。它不意味着把所有内部能力外放；四组 truth blocker 先各自通过门禁，然后 transport 在同一提交切换。

`memory.create` 首先只支持 Working Memory，返回真实 `entry_id` 和 embedding state。不要让现有 `Remember`（Ephemeral 且无 ID）冒充该能力，也不要让 tool handler 为 Working Memory 返回空 ID。

### 内部依赖

```text
public_service
  ├── RequestContext(authenticated AgentRole)
  ├── memory domain (LayeredMemory + persister)
  │      └── indexing pipeline (derived, eventual)
  ├── object domain (SemanticFS + CAS)
  ├── session/checkpoint/event watermark
  └── readiness/capability catalog
```

### 已完成实现记录

1. `remember_working_scoped_with_id` 已提升为 public service 可调用的 crate 内 typed primitive，不再丢弃 ID。
2. owner/role-scoped memory entry read 已统一过滤 deleted/superseded，并由 typed DTO 暴露真实状态。
3. update/delete 已返回 typed result；late indexing task 使用 `set_embedding_if_pending` 防止复活旧 entry。
4. V1-A recall 只返回同域 lexical overlap；后续 hybrid 合并 Ready vector candidates 时返回 entry IDs、score components、`matched_by` 和 degradation，不复用以 CID 为键的 object BM25 分数。
5. `index_status` 只返回真实三态；last error/attempt/queue depth 在可靠 per-entry job record 建立前继续不公开。
6. V1-A capabilities 只报告 lexical recall 可用性和已记录的 embedding worker/provider 状态，不主动探测 provider，也不宣称 hybrid。只有 hybrid 经消融与 ADR 接纳后，后续 capability 才能报告其 degradation；不得以静默 lexical fallback 冒充 hybrid 成功。
7. session v1 只包含真实 session/delta/watermark；warm placeholder、prefetch 和假 checkpoint 已从公共响应排除。

### BM25 决策工作流

| 领域 | 当前状态 | 计划裁决 |
|---|---|---|
| Object | 已有 CID-keyed `Bm25Index` 与 vector RRF | V1-A 只在路径诊断、filter 与 degradation 门禁通过后公开；update/delete/history 仍不公开 |
| Memory | 无 BM25；当前是 entry-ID-keyed `lexical_overlap` | V1-A 只公开 lexical。新增 Memory BM25 必须作为研究候选，不列入 P1 承诺 |
| Projection manifest | 尚未实现 | 只有 Memory BM25 通过消融并经 ADR 接受后，P3 才为其建立 builder/artifact/watermark |
| Thermal | 尚未实现 | 不预设 BM25 常驻哪一层；用 DECAY gate 比较 postings、压缩目录或其他无模型 discovery 结构 |

Memory BM25 消融必须在相同 canonical entry/revision、active filter、role filter、tokenizer 和预算下比较 `lexical_overlap`、BM25-only、vector-only、hybrid，至少报告 recall@k、MRR、CJK/精确实体召回、写后可见延迟、p95/p99、内存/磁盘、rebuild cost。只有收益超过预注册阈值且不破坏 canonical-first/read-after-write 才提交 ADR；否则删除候选实现。

### P1 benchmark gate

- canonical write acknowledgement p50/p95/p99 与 embedding provider 延迟分离；
- provider 正常、超时、错误、invalid vector 四种 fixture；
- 单写后立即 lexical hit = 100%；
- 可索引 entry 最终 `Ready`；provider 失败时 canonical 可读且保持 Pending；
- queue saturation 不丢 entry，30 秒协调周期后恢复；
- daemon restart 后 pending private memory 恢复；
- update/delete 后旧 task 不覆盖或复活旧 entry；
- typed recall entry-ID recall@k，不使用 substring proxy；
- role isolation leak rate = 0；
- UDS/TCP/MCP 对同一 fixture 返回等价 typed payload；
- benchmark 原始样本、run ID、dataset SHA 和环境可复现，失败 fail closed。

上述正确性与 transport gate 已作为 P1 关闭条件通过。p50/p95/p99、真实 embedding 模型质量与资源曲线仍属于持续 benchmark，不得用 stub 结果外推；它们不重新打开已完成的 typed truth loop，但会约束后续 retrieval 候选是否可接纳。

## 历史完成切片：V1-B Canonical Truth Firewall

V1-B 的唯一目标是把已经真实工作的 Working Memory 闭环迁移到稳定 logical identity 和 append-only revision ledger。详细语义与硬门禁见 [ADR-0004](../adr/0004-canonical-revision-ledger.md)。本切片不扩展现有 13 项 public operation，不提前加入 history、restore、evidence、projection、thermal 或 hard erase。

### 单路径实施顺序

1. 冻结 `memory_id`、revision identity、`parent_revision_id` 与 `canonical_content_hash`；Structured JSON hash 使用标准 JCS 库和 golden vectors，不自制 JSON 规范化。
2. create 追加 root revision；update 追加共享 `memory_id` 的 child revision；delete 追加 tombstone revision；三者都保持旧 revision bytes 不变。
3. current head、superseded 与 deleted 视图只从 revision chain 派生；生产代码不再修改旧 entry 的 `superseded_by/deleted_at`。
4. 审计 `store/delete/clear/persist_memories/compact/restore` 全部调用点，破坏历史或绕过 durable commit 的路径重写后物理删除。
5. 以停写、备份、dry-run、数量/hash 校验和原子 root 切换完成旧 snapshot 一次性迁移；迁移后删除旧 reader/writer，不保留 runtime fallback、alias、adapter 或双写。
6. 结构化 trace 覆盖 `validate -> load_head -> construct_commit -> persist_ledger -> publish_current_view -> enqueue_projection`，只记录 ID、阶段、水位、字节数、稳定类别和耗时，不记录正文、凭据、完整 hash 或私有路径。

### V1-B 团队门禁

- 架构组：ADR-0004、canonical/runtime/projection 字段矩阵、普通 delete 与 hard erase 边界冻结；
- 开发组：唯一 revision writer、current-view rebuild、一次性 migrator 和旧破坏路径删除；
- 测试组：immutability、hash golden vector、并发 expected-head、persist/publish/enqueue fault injection、restart 和 migration rejection；
- 审计组：证明旧 writer/reader、原地状态修改、破坏性 compact 和兼容壳的生产调用为零；
- 研究组：只准备 lexical/BM25/vector/hybrid 消融，候选索引不得接入 canonical writer；
- 专家组：全部硬门禁通过才允许 Projection Manifest 实施；任何数据丢失、双 head、旧 revision 改写或 migration 猜测都直接 no-go。

V1-B 的完成判定以 ADR-0004 的 12 项硬门禁为准，不以新增字段、单元测试数量或 happy-path demo 宣布完成。

### 2026-08-13 基础切片审计（历史状态）

本轮已建立 typed identity/hash、JCS、Working create/update/tombstone 的候选-persist-publish原语、snapshot revision graph 验证，并物理删除 `memory_move`、破坏性 compact/clear/checkpoint restore 路径。benchmark 的单次实验真实性 manifest/failure ledger 已通过审计。

该审计当时的 **NO-GO for existing vaults** 已由后续实现关闭：offline-only `inspect/dry-run/migrate`、verified backup、immutable segments、typed seal、`RENAME_EXCHANGE`、交换后复核、失败回滚和 post-migration restart 均已落地。迁移只接受 sealed、preflight 通过、受支持的线性历史；坏链、causal relation 和未映射 Group 继续 fail-closed，不猜测修复。

2026-08-14 独立发布审计将 V1-B 判为 Completed。fresh vault、Private/Shared cutoff/显式 Group 迁移、逐 stream policy、重启、六类写断连无重放及真实 UDS/LLM dogfood 已通过。单次 release run 只证明正确性与本机观测值，不支持性能优越性结论。下一生产切片是 P3-A Projection Manifest，不扩 public API，也不提前实现 thermal。

## P2-B：Evidence、稳定记忆身份与 Claim（领域编号；P3-A 后实施）

### V1-B 迁移前置

引入 `memory_id` 与 `revision_id` 是持久化语义变化。P2-B 只能建立在已经完成的 V1-B 离线迁移上：

1. 备份并校验现有 persistence index；
2. 定义旧 `MemoryEntry::id` 到 revision 的一次性映射；
3. 运行离线 migrator，验证数量、内容 hash、supersedes/deleted 链；
4. 切换后删除运行时旧格式读取和双写。

### 公共 API

- `evidence.ingest/get`；
- `memory.history/correct`；
- `claim.propose/review/history`。

### 领域约束

- evidence 保存 source locator、observed/ingested time，CID 终止 provenance；
- claim 明确区分 source assertion、model inference、user confirmation；
- 高影响身份/偏好/遗忘变更必须 review；
- “更新更晚”不是自动正确；valid time 与 observed time 分离；
- dedup/merge 输出 proposal，不静默吞掉 canonical write。

### P2 benchmark gate

- 同源同 revision 摄取幂等；不同源相同字节共享 CID 但 provenance 不丢；
- 每个 accepted claim 至少一条可解析 evidence/user-confirmation edge；
- correction 后当前视图正确，旧 revision 可按时间恢复；
- 冲突 fixture 保留双方证据，未 review 不自动覆盖；
- deletion/retention 与 epistemic/thermal 状态正交；
- LongMemEval/LoCoMo 增加 temporal update、preference correction 和 abstention 子集。

## P3-A：Projection Manifest 单一控制面（V1-B 后的下一生产能力）

状态：Active。冻结 schema、状态机、CAS/锁边界和公共切换见 ADR-0005。Rust 生产路径已经
破坏式切到 `plico.personal.v2` exact-14，manifest 是 `memory_embedding` 的唯一状态源；B2 的
benchmark、外部 demo、真实 dogfood 与发布 artifact 尚未全部封板，因此本阶段仍不得标为 Completed。

本切片只准入 `memory_embedding`。这里的“单一”表示它是该准入 kind 的唯一状态源；Object HNSW、
Object BM25、Memory BM25/KG/summary 均不在本切片。P3-A 只交付控制面与耐久 artifact，不激活
Memory vector/hybrid recall；public recall 继续精确使用 `lexical_overlap`。

### 实施顺序

1. 把 CAS 锁所有权收敛为一个 `PersonalVaultStorage` 生命周期锁，再提供固定的 memory-ledger、
   单一 `projection-store/{manifest,artifacts}` 生命周期与 object CAS handle；projection fresh bootstrap 使用
   sealed staging + whole-parent `NOREPLACE`，owner reset 使用 Prepared/Applied marker、whole-parent
   `RENAME_EXCHANGE` 与 0700 quarantine recovery；禁止第二把 vault lock、旧 sibling reader、live partial
   create、递归删除不可信旧树或非 CAS 文件 I/O；
2. 实现 ADR-0005 的 append-only records/segment/root/current-view、六态转换、完整 canonical
   watermark、artifact durable-before-root 原子可见边界和 replay validator；
3. 只为 `memory_embedding` 建 stable builder spec、durable lease/retry 和 reconciler；内存 queue 只是
   可丢通知；
4. V1-B canonical 没有 inline durable embedding。一次性切换不读取 runtime vector，而是从 verified
   canonical root 为每条 revision 全量分类，eligible revision 进入 Queued 后重建 artifact；
5. 同一切换中让 builder、reconcile、persist、restore、status 只使用 manifest，并物理删除
   `MemoryEntry.embedding`、`embedding_state()` 三态、runtime-only retry truth、Memory vector reader；
6. 同时删除 LongTerm write-before-commit embedding/silent dedup，改为 canonical-first 无条件追加；
   reconciliation proposal 在 P2-B 前 unsupported；
7. 用 deterministic builder 全量 rebuild，比较 canonical bytes/root 不变、revision↔manifest 双射、
   eligible/Ready/Absent 数量、artifact hash 集合和 watermark；
8. 本仓 transport/client 已一次切到 personal.v2；benchmark 与外部 demo 必须完成同样破坏式迁移，
   并用真实 dogfood 证明 exact-14 后才封板。任何 v1 reader、alias、enum 映射或双轨 store 都禁止。

不得先加 manifest 再长期双写内嵌 embedding。

### 公共 API

- `plico.personal.v2` 保留原 13 项中的 12 个名字，删除 `memory.index_status`，新增
  `projection.status`、owner-only `projection.rebuild`，精确 catalog 为 14 项；
- 删除 MemoryEntryView 的 `embedding_state`。`projection.status` 区分
  `observed/unreconciled/unavailable`；observed 状态为
  `Queued/Building/Ready/Failed/Stale/AbsentByPolicy`，不得把 Failed/Stale 压成 Pending/Ready；
- `projection.rebuild` 成功只表示 Queued transition/root 已 durable，不表示构建完成，写调用不自动
  retry；
- recall response 可以增加真实 canonical/projection watermark 与 degradation，但在 vector gate 前仍只
  报 `matched_by=lexical_overlap`；
- 鉴权后的 typed status 可以返回完整 content hash 绑定 revision identity；trace/log/metric/error/
  未鉴权诊断禁止记录完整 content/root/artifact hash；
- 当前 capabilities 声明 `memory_embedding.control_plane=supported`，并继续声明
  `memory_embedding.retrieval`、Memory vector/hybrid/BM25 为 unsupported。

### P3 benchmark gate

- 删除 projection manifest/artifact store 后可从 canonical 完整重建；
- manifest coverage 内每条 canonical revision 与 entry 恰好一一对应；
- model/dimension/builder spec 变化在单个 root generation 中标 Stale，不把旧向量当 Ready；
- crash 在 Queued、Building、artifact durable、root exchange、post-exchange fsync 任一点均可协调；
- enqueue 失败不回滚 canonical，status 诚实为 unreconciled；manifest 损坏不阻断 lexical recall；
- canonical revision bytes/hash/root chain 在 rebuild 前后相等；deterministic builder 的 artifact hash
  集合一致；
- ACL/owner rebuild 边界在 restart 后不变，未授权 status 不泄露存在性；
- `MemoryEntry.embedding`、三态推导、runtime retry truth、v1 status reader 和其他
  `memory_embedding` 状态源全部为零；
- personal.v2 exact 14-op、UDS/TCP/MCP/aicli/plico-agents、故障注入、redacted trace 和真实 dogfood
  同 run evidence 全通过。

## P4：可逆记忆腐败与 deep recall

P4 是通过 DECAY 实验后才可接受的候选阶段，不是因为提出了 Hot/Warm/Cold/Dormant 就必然实施。若消融表明更简单的两层 projection residency 达到相同召回/成本目标，架构组应修改 ADR 并采用更简单模型。

### 公共 API

- recall `fast/balanced/deep`；
- `thermal.pin/unpin/status`；
- `rehydrate.status/cancel`。

### 内部依赖

- P3 projection manifest；
- 独立 Cold projection store；
- 与 embedding 模型无关的 Dormant discovery directory；
- 有界、持久、可取消 rehydrate jobs；
- verified-use feedback，不把候选扫描计 hit。

### P4 benchmark gate

- 执行 `benchmarks/docs/personal-memory-evaluation-gates.md` 的 DECAY-01..12；
- fixed clock 下 Hot→Warm→Cold→Dormant 可重复；
- 每个 Dormant fixture 可由 deep 找到同一 canonical revision；
- fast miss 明确返回 coverage，不表述为不存在；
- deep latency、bytes、rehydrate 与 hot latency 分开报告；
- temperature 变化前后 canonical/evidence hash、epistemic state、importance 不变；
- deep hit 先回 Warm，只有 verified repeated hit 或 pin 进入 Hot。

## P5：人类侧按需投影

### 公共 API

- `projection.generate`：document/table/slides/chart；
- `projection.get/export/delete`；
- `projection.diff`、`projection.absorb`。

### 约束

- 每个 artifact 绑定 memory revision、evidence CID、builder/template/model version 和时间；
- delete artifact 不删除 canonical；
- generate/rebuild 不改变事实置信度；
- 文件修改只有显式 absorb 才成为新 evidence/revision proposal；
- AI 默认直接使用 recall/memory/evidence，不重复解析自身生成文档。

### P5 benchmark gate

- 相同 frozen inputs + builder version 可复现 manifest；
- 文档、表格、PPT 中的 claim 可追溯到 source revision/evidence；
- 删除并重建 artifact 后 canonical 不变；
- 编辑导出文件不触发隐式 memory mutation；
- absorb 有 diff、review、new evidence/revision 事件。

## 删除与内部化执行清单

### P0 删除状态

- 已物理删除：tenant management request/DTO/handler/store、cluster/distributed、CAS `evict_cold` public path、public group recall、MCP 虚假说明、无消费者的 SSE/A2A binary；
- 已物理删除：duplicate core/poly public verbs、aicli legacy command、SSE 配置残留、generic MCP passthrough 与 universal response 无生产方字段；
- public cutover 已删除：public request `tenant_id`、variant 内零散 token/agent identity 和旧 serde transport；
- V1-B 迁移后删除：旧 memory snapshot reader/writer、`supersedes/superseded_by` 状态源、对旧 revision 的 `deleted_at` 修改、破坏历史的 compact 与 restore-clear 路径；
- 持久化迁移后删除：storage `tenant_id`、历史 `MemoryScope::Group`、旧 permission snapshot 中已删除枚举值与孤儿 index；
- 持续删除：失实里程碑、硬编码 benchmark 历史结论、无真实 handler 的 capability 说明。

### 一次性迁移 preflight

1. 只读扫描 CAS object metadata、memory snapshots、KG redb、checkpoint/passport、permission snapshot、`tenant_index.json` 与 recycle/audit state，输出数量和 hash manifest；
2. 对未知/已删除 enum 值 fail closed，不能整份恢复为空后继续启动；
3. `MemoryScope::Group` 的目标可见性由独立 ADR 决定，migrator 不设默认映射；
4. 历史 cold eviction 的 soft-delete 恢复与普通用户删除分开审计；
5. 每项迁移先备份、dry-run、数量/hash 校验，切换后删除旧读取器和双写路径。

### 内部化候选

- KG maintenance、scheduler、agent messaging/delegation；
- permissions/admin、hooks/prompts/model/cache；
- raw traces/events、deep diagnostics；
- file import、memory tier maintenance；
- skill forge 与 cognition internals。

### 审计方法

每个候选必须记录：定义、生产调用方、测试调用方、transport 路径、持久化依赖、删除决定。只有测试调用或说明文字不构成生产调用方。确认无生产方后一次删除类型、handler、route、test fixture、文档，不保留空实现。

## 专家组需要持续裁决的问题

1. LongTerm semantic dedup 从写时合并改为 reconciliation proposal 的具体用户确认策略；
2. V1-B ledger segment/checkpoint 的性能预算与实现选择；稳定 identity、parent、hash 和 append-only 语义已由 ADR-0004 冻结；
3. Evidence source locator 的隐私分级和 export redaction；
4. AgentRole 最小集合与 local-owner UDS 映射；
5. 哪些 session prefetch 能以真实水位和 job state 对外；
6. Cold store 的加密、容量预算和无模型 discovery format；
7. 人类投影 absorb 的确认粒度。
8. Memory BM25 相对 lexical overlap/vector 的预注册收益阈值，以及是否值得进入 projection manifest；

已冻结、不再作为开放问题：projection artifact 必须先 durable/verified，随后 manifest root 以同 vault
lock 下的双槽 `RENAME_EXCHANGE` 原子发布；crash 只允许产生不可见 orphan，详见 ADR-0005。历史
`MemoryScope::Group` 的离线迁移语义已由 ADR-0004 与 V1-B migration manifest 冻结。

## 已完成的 V1-B 交付定义

P0、P1、V1-A 与 V1-B 已完成。V1-B 不是以“新增了字段或 API variant”为完成，而是由以下同时满足的结果关闭：

- 已有外部 AI truth loop 在 authenticated public protocol 上继续通过，不因内部 ledger 迁移改变 13 项 contract；
- create/update/delete 分别追加 root/content/tombstone revision，`memory_id` 稳定且旧 revision bytes/hash 不变；
- current view 在 restart、persist/publish/enqueue failure 后可从唯一 ledger 恢复；
- 一次性迁移对坏链 fail closed，对有效 fixture 保持数量、内容与状态；
- compact、TTL、projection maintenance 和 restore 不物理删除 canonical history；
- 未实现的 history/restore/hard erase/evidence/claim/projection/thermal 继续明确 unsupported；
- benchmark 使用真实 ID、真实结果、真实失败语义、run manifest 和 failure ledger 验收；
- 没有旧格式 runtime reader/writer、兼容壳、双轨状态、假字段或谎报成功。

历史 V1-B release/dogfood bundle 由独立的完整 release artifact 保存，本源码仓库不提交本机 trace 或不完整 bundle。它们只证明当时的 personal.v1/V1-B 边界，不能替代 P3-A personal.v2 exact-14 的新证。P3-A 封板前仍不宣称 Memory vector/hybrid retrieval，更不启动 thermal active path。
