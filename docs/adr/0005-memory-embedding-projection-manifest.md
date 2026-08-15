# ADR-0005：Memory Embedding Projection Manifest 单一控制面

- 状态：Proposed（代码切换、迁移和 P3 门禁完成前不得改为 Accepted/Completed）
- 日期：2026-08-14
- 上位决策：[ADR-0002：Canonical Memory、按需投影与可逆检索腐败](./0002-canonical-memory-and-reversible-retrieval-decay.md)
- Canonical 依据：[ADR-0004：Canonical Revision Ledger](./0004-canonical-revision-ledger.md)
- 公共契约：[ADR-0003：个人数字分身统一领域模型与公共能力契约](./0003-personal-twin-domain-and-public-capability-contract.md)
- 实施计划：[个人数字分身公共能力演进计划](../plans/personal-twin-public-capability-evolution.md)

## 背景与当前事实

V1-B 后，`CanonicalRevision` 已经是 durable memory 的唯一事实来源，并且不包含
embedding。当前 embedding 路径仍有以下互相冲突的运行时状态：

- `MemoryEntry.embedding: Option<Vec<f32>>` 保存进程内向量；
- `embedding_state()` 从 `None/Some` 推导 `NotRequested/Pending/Ready`；
- indexing pipeline 的 queue、in-flight、retry 和协调状态只存在于内存；
- canonical replay 会把每条 revision 的 embedding 重置为 `None`，因此重启后重新推导
  Pending；
- provider/model/dimension 热切换没有 durable builder identity，也不会可靠地把旧向量
  标为 Stale；
- LongTerm 内部写路径在 canonical commit 前同步计算 embedding，并可能因相似度而静默吞掉
  canonical write。

这些字段可以支持第一阶段的 eventual indexing，却不能证明 artifact 属于哪条 immutable
revision、由哪个 builder 生成、是否跨重启 Ready、失败是否可协调，也不能表达 Stale 或
AbsentByPolicy。继续在旧字段旁增加 manifest 会形成双真值，本 ADR 禁止这种迁移方式。

另有三个必须明确的范围事实：

1. V1-B canonical 中没有可迁移的内嵌向量；P3-A 只能从 canonical 全量重建，不能把进程内
   向量伪装成 durable migration source。
2. 当前 Object 域的 HNSW、`.embedding_meta.json` 和 BM25 是另一套历史 projection 实现。
   本切片不迁移、不改写、不通过新 manifest API 宣称它们。
3. public `memory.recall` 当前只执行 `lexical_overlap`。建立 Ready embedding artifact 不自动
   证明 vector recall 的质量或退化语义已经通过门禁。

## 决策摘要

P3-A 建立一个 append-only、可重放、可原子发布的 Projection Manifest。它是**已经准入的
projection kind 的唯一状态源**。本切片唯一准入的 kind 是 `memory_embedding`；所有未知 kind
一律 fail closed。

Manifest 与 artifact 都是可删除、可重建的派生数据。Canonical ledger 仍是唯一事实来源；
manifest 的任何失败都不能回滚已经 durable 的 canonical commit，也不能阻止授权调用方使用
`memory.get` 或 lexical recall。

P3-A 只交付 embedding 控制面、耐久 artifact、水位和协调闭环，不激活 Memory vector/hybrid
recall。`Ready` 只表示指定 canonical revision 与 builder spec 的 artifact 已耐久写入并通过
验证，不表示该 artifact 已被召回策略使用。

## 1. 准入范围与唯一状态源

### 1.1 唯一准入 kind

`ProjectionKind` 在 v1 schema 中只有：

```text
memory_embedding
```

其 projection policy 精确为：

- source 必须是 canonical memory revision；
- 只有当前 active、未删除、非空 `Text`，且 cognitive tier 为 Working 或 LongTerm 的 revision
  才需要 artifact；
- superseded revision、tombstone、Ephemeral、Procedural、非文本和空文本均为
  `AbsentByPolicy`；
- ACL 不复制到 manifest。每次 status/artifact 访问都用 canonical ledger 的**当前** policy
  鉴权，避免 projection 中产生第二份权限真值。

“单一 manifest”在 P3-A 中严格表示：对 `memory_embedding`，除 manifest 外不存在第二个 durable
或 runtime 状态源。Object HNSW/BM25 尚未准入此控制面，因此不能用 P3-A 的状态/API/水位描述，
后续若要准入必须另行修订 ADR 并进行一次性切换。尤其，legacy Object HNSW metadata 不绑定
本节的 compatibility digest；它不能据此声称 provider 变更后的向量正确性，也不支持模型切换。

### 1.2 必须删除的旧状态

同一次代码切换中物理删除：

- `MemoryEntry.embedding`；
- `embedding_state()` 的三态推导及所有以 `None` 推断 Pending 的调用点；
- memory embedding 的 runtime-only retry/in-flight 状态真值；内存 queue 只能是可丢通知，不能
  决定状态；
- 从 runtime entry 直接读取向量的 memory semantic recall 路径；
- LongTerm 写前 embedding 和 silent semantic dedup。LongTerm 改为 canonical-first、无条件追加
  revision；dedup 只能在未来 P2-B 形成 reconciliation proposal，在此之前明确 unsupported。

不得保留旧字段做 fallback、adapter、shadow read 或双写。

## 2. Identity、watermark 与 builder 绑定

### 2.1 Projection identity

每条 canonical revision 对每个准入 kind 最多有一个 projection stream。`projection_id` 使用 UUIDv5：

```text
namespace = UUIDv5(UUID_NAMESPACE_URL,
                   "https://plico.ai/schema/projection/v1")
name      = "memory_embedding\0" || revision_id
```

`revision_id` 使用 UUID 的 lowercase hyphenated ASCII 表示，name bytes 不含终止 NUL；上述内嵌
`\0` 是 projection kind 与 revision identity 的唯一分隔字节。

`projection_id` 不包含 model、dimension 或 builder spec。模型升级推进同一 projection stream 的状态，
不得通过生成新 ID 让旧 Ready artifact 继续被误用。

### 2.2 Canonical source identity

每个 projection entry 必须绑定：

```json
{
  "canonical_kind": "memory_revision",
  "memory_id": "uuid",
  "revision_id": "uuid",
  "revision_sequence": 42,
  "content_hash": "64-char-lowercase-sha256"
}
```

`memory_id`、`revision_id`、`revision_sequence` 和 `content_hash` 必须与已验证的 canonical ledger
record 精确相等。Revision 是 immutable source；更新会产生新的 projection stream，旧 stream 按
policy 转为 `AbsentByPolicy`，不能把旧 artifact 改绑到新 revision。

### 2.3 Canonical watermark

Manifest 的 reconciliation coverage 使用完整 watermark，而不是单独的 revision count：

```json
{
  "root_hash": "64-char-lowercase-sha256",
  "generation": 17,
  "revision_watermark": 81,
  "policy_watermark": 30,
  "relation_watermark": 4
}
```

`root_hash` 必须指向可由 canonical ledger 完整重放的 root，generation 与三个 watermark 必须与
该 root 相等。新的 reconciled watermark 必须位于旧 watermark 的 canonical root 后代链，禁止
只比较数字后跳到无关 root。

Watermark 表示“截至该 canonical root 的每条 revision 已被分类并写入 manifest”，不表示全部
artifact 已 Ready。Ready coverage 由各状态数量单独报告。Policy/relation 变化通常不要求重建
embedding，但 reconciliation 仍可前进完整 watermark；访问控制始终查询最新 canonical policy。

### 2.4 BuilderSpec

Builder spec 是 immutable、JCS canonical 的记录，至少包含：

```json
{
  "schema": "plico.projection.builder-spec/v1",
  "projection_kind": "memory_embedding",
  "builder_id": "plico.memory-embedding",
  "builder_version": "immutable-implementation-revision",
  "provider_family": "stable-non-secret-family",
  "provider_compatibility_id": "stable-non-secret-output-contract-id",
  "model_id": "model-name-or-id",
  "raw_dimension": 768,
  "dimension": 384,
  "input_contract": "memory_text_utf8_v1",
  "operation_contract": "document_v1",
  "normalization": "l2_after_matryoshka_truncation_v1",
  "transform_contract_id": "plico-matryoshka-truncate-l2-v1",
  "artifact_schema": "plico.projection.embedding-artifact/v1"
}
```

Hash domain 为 `plico.projection.builder-spec.v1\0`。完整 provider URL、token、请求体和原始 provider
错误不得进入 spec。Provider 必须提供一个稳定、非秘密的 compatibility ID，并保证 model
revision、预处理、输出维度或归一化语义变化时该 ID 改变；无法提供稳定 identity 时不得生成
或激活 BuilderSpec，projection control plane 报 unavailable，且不得伪造或持久化 Failed attempt。
`builder_identity_unavailable` 是激活前的 subsystem health，不是 `Failed.failure_category`。已经有 active
spec、但对应 provider 暂时无法实例化或服务时，构建 attempt 才能进入稳定类别
`provider_unavailable` 的 Failed。

字段 wire 值冻结如下：`input_contract=memory_text_utf8_v1`、
`operation_contract=document_v1`；`normalization` 仅允许 `provider_native` 或
`l2_after_matryoshka_truncation_v1`。前者要求 `raw_dimension == dimension` 且
`transform_contract_id=provider-native-document-v1`；后者要求 `raw_dimension > dimension` 且
`transform_contract_id=plico-matryoshka-truncate-l2-v1`。`transform_contract_id` 只能引用仓内已知、
非秘密的完整 document transform contract；禁止保存 raw prefix、provider request 或低熵 prefix hash。
自定义 transform 无可信稳定 contract ID 时同样是 builder identity unavailable。`dimension` 是 artifact
effective dimension，`raw_dimension` 是 provider 原始输出 dimension；两者均为 1..=65536。

Model、dimension、builder version 或 compatibility ID 任一变化都会产生新 spec hash，并在同一次
manifest root 提交中激活新 spec、把旧 spec 的 Ready artifact 标为 Stale。旧 artifact 不再
servable，随后由协调器排队重建。

当前 provider capability 冻结如下：

| Provider | Legacy Object embedding | P3 `memory_embedding` identity |
|---|---|---|
| Ollama | 可用 | 唯一可准入；要求显式 full tag、规范 64-hex digest、服务版本、`/api/embed` `truncate=false`，并在每次 document build 前后复验 digest/version |
| OpenAI-compatible | 可用 | unavailable；alias、operator pin 与一次 shape probe 不能证明远端 immutable revision |
| Local Python worker | 可用 | unavailable；当前标准 Hugging Face snapshot 的实际加载字节尚无完整、无 symlink/TOCTOU 歧义的 bundle proof |
| Stub | 仅显式测试/tag-only | unavailable；禁止冒充生产 builder |
| ORT | 已删除 | activation unavailable；旧实现硬编码 model/dimension/pooling/max-length，不能形成可信 bundle contract |

Operator expected pin 只能比较 provider 实际证明出的 identity，不能制造 identity。OpenAI、Local、
Stub 或已删除的 ORT 都不得生成 BuilderSpec；系统在 activation 前返回稳定
`builder_identity_unavailable`，不能降级成伪 Ready、伪 Failed 或 Stub vector。

## 3. 唯一 manifest schema

所有 JSON 类型使用 `deny_unknown_fields` 等价规则、RFC 8785/JCS canonical bytes、严格 schema
字符串、非 nil UUID、严格递增 sequence、严格排序且无重复的集合。禁止手写 JSON canonicalizer。

### 3.1 ManifestRecord

全局日志记录：

```json
{
  "schema": "plico.projection.manifest-record/v1",
  "sequence": 12,
  "committed_at": 1786636800000,
  "committed_by_role": "projection-worker",
  "event": { "type": "projection_transition", "...": "tagged payload" }
}
```

`event` 只有三类：

```json
{
  "type": "builder_activated",
  "projection_kind": "memory_embedding",
  "builder_spec": {},
  "builder_spec_hash": "sha256",
  "previous_builder_spec_hash": "sha256-or-null"
}
```

```json
{
  "type": "projection_transition",
  "projection_id": "uuid",
  "projection_kind": "memory_embedding",
  "projection_version": 2,
  "previous_sequence": 7,
  "source": {},
  "desired_builder_spec_hash": "sha256",
  "state": { "type": "queued", "reason": "reconciliation" }
}
```

```json
{
  "type": "reconciliation_advanced",
  "previous_source": {},
  "reconciled_source": {},
  "classified_revision_count": 81
}
```

第一次 builder activation 的 `previous_builder_spec_hash` 为 null；之后必须精确等于当前 active spec。
第一次 projection transition 的 `previous_sequence` 为 null、version 为 1；之后必须精确链接同一
projection 的上一条 transition。第一次 reconciliation 的 `previous_source` 指向 canonical genesis
root；之后必须等于 CurrentView 的旧 source watermark。

除 genesis 外，全局 sequence 从 1 连续递增。每个 projection stream 的 version 从 1 连续递增，
`previous_sequence` 必须精确指向同一 projection 的上一条 transition。Builder activation、相关
Stale transitions 和 reconciliation 变更需要在同一个 segment/root generation 中原子可见。

`committed_by_role` 只能来自可信执行上下文：owner 显式 rebuild/spec 变更为
`personal-owner`，自动 enqueue/build/reconcile 为保留内部角色 `projection-worker`。请求 payload
不得自报 actor。Manifest 不引入 tenant、organization、workspace 或共同所有者字段。

`committed_at` 同样不能由请求、worker payload 或 controller 自报。唯一 store writer 使用可信系统时钟
并盖章 `max(system_now_ms, previous_root.committed_at + 1)`，以 checked arithmetic 保证每个 root generation
严格递增；测试 lease/backoff 通过 crate-private clock 注入，不用 sleep。一个 generation 的全部新增 record
必须与 root 的 `committed_at`/`committed_by_role` 精确相等，且 event watermark 必须严格增加。

### 3.2 状态 payload

Manifest 持久状态精确为：

| 状态 | 必需 payload | 含义 |
|---|---|---|
| `AbsentByPolicy` | 稳定 `reason` | 按准入 policy 不应有 artifact |
| `Queued` | 稳定 `reason` | 已耐久请求构建，尚无 lease |
| `Building` | `attempt`、随机 `attempt_id`、`lease_expires_at` | 某次有界构建已耐久 claim |
| `Ready` | `attempt`、`attempt_id`、`ArtifactDescriptor` | artifact 已先耐久写入、校验，再由 root 发布 |
| `Failed` | `attempt`、`attempt_id`、稳定 `failure_category`、`retryable`、可选 `retry_not_before` | 本次构建失败；不保存原始 provider 错误 |
| `Stale` | 稳定 `reason`、旧 `ArtifactDescriptor` | artifact 存在但不匹配当前 desired spec/source，永不 servable |

`attempt` 只在进入 Building 时加一。`attempt_id` 必须在 Building→Ready/Failed 时精确匹配，旧 worker
结果不能提交。`lease_expires_at` 到期后，reconciler 追加 Building→Queued；不得原地修改 Building。
Retryable Failed 的 worker Retry 只有到期后才能追加 Failed→Queued；worker 不得用 Reconciliation、
CanonicalCommit 或 LeaseExpired 绕过 backoff。无论 retryable 与否，owner 都可用 BuilderChanged 或
OwnerRebuild 立即离开 Failed，以免紧急 spec 切换被单个失败 stream 卡死。Owner rebuild 在一个 root
generation 中追加 Stale（若存在 artifact）和 Queued，成功仅表示 Queued 已 durable，不表示构建完成。

允许的主要转换：

```text
AbsentByPolicy -> Queued
Queued          -> Building | AbsentByPolicy
Building        -> Ready | Failed | Queued(lease expired) | AbsentByPolicy
Ready           -> Stale | AbsentByPolicy
Failed          -> Queued | AbsentByPolicy
Stale           -> Queued | AbsentByPolicy
```

不允许 Ready 直接覆盖为另一份 Ready，不允许 artifact publish 绕过 Building，不允许状态就地改写。

状态原因/类别是 schema enum，不接受任意 provider/storage 字符串：

```text
AbsentByPolicy.reason = superseded | deleted | unsupported_tier |
                        unsupported_content | blank_text
Queued.reason          = canonical_commit | reconciliation | retry |
                        owner_rebuild | builder_changed | lease_expired
Stale.reason           = builder_spec_changed | artifact_missing |
                        artifact_hash_mismatch | artifact_invalid | owner_rebuild
Failed.failure_category = provider_unavailable | provider_identity_changed |
                          invalid_projection | artifact_store_unavailable
```

Retryable Failed 必须有 `retry_not_before > committed_at`；non-retryable Failed 必须令
`retry_not_before=null`。原始错误仅存在于被 redaction 的进程内诊断，不持久化、不返回、不作为 label。
`provider_identity_changed` 是 non-retryable、无 retry timestamp 的 restart-required 事实；同一进程不得继续
用旧 sealed builder 排队或构建。普通 provider 调用失败才是 retryable 的 `provider_unavailable`。

### 3.3 Segment、Root 与 CurrentView

`ProjectionSegment`：

```json
{
  "schema": "plico.projection.manifest-segment/v1",
  "first_sequence": 1,
  "last_sequence": 12,
  "previous_segment_hash": null,
  "records": []
}
```

`ProjectionRoot`：

```json
{
  "schema": "plico.projection.manifest-root/v1",
  "generation": 3,
  "previous_root_hash": "sha256-or-null",
  "manifest_head": "sha256-or-null",
  "event_watermark": 12,
  "current_view_hash": "sha256",
  "reconciled_source": { "...": "CanonicalWatermark" },
  "committed_at": 1786636800000,
  "committed_by_role": "projection-worker"
}
```

Genesis root 的 generation=0、previous/root head=null、event watermark=0，CurrentView 为空并把
reconciled source 绑定 canonical genesis root。后续 root generation 必须逐一加一；只有 genesis 的
previous root 可为 null，event watermark 不回退，previous root chain 必须完整可重放。第一次
builder activation 之后，generation>=1 的 active builder 对 `memory_embedding` 必须恰好一个；
activation 前 projection control plane 只能报告 unavailable，不能构造默认 spec。

`ProjectionCurrentView`：

```json
{
  "schema": "plico.projection.current-view/v1",
  "generation": 3,
  "event_watermark": 12,
  "reconciled_source": { "...": "CanonicalWatermark" },
  "active_builder_specs": [],
  "entries": []
}
```

其中每个 `entries` 元素精确为：

```json
{
  "projection_id": "uuid",
  "projection_kind": "memory_embedding",
  "projection_version": 2,
  "last_transition_sequence": 11,
  "attempt_count": 1,
  "source": {},
  "desired_builder_spec_hash": "sha256",
  "state": { "type": "ready", "...": "state payload" }
}
```

CurrentView 的 state payload 与 manifest transition 使用同一个 tagged type；不得另建一套状态 DTO
或从 artifact 是否存在反推状态。

CurrentView 不含 root hash，保持 `root -> current_view` 的单向 DAG，禁止同代循环 hash。View 必须
由全部 records 确定性重放得到，raw JCS bytes 与重放结果逐字节相等。`active_builder_specs` 按 kind
排序；builder activation 后每个准入 kind 恰好一个。Entries 按 `(projection_kind, revision_id)` 排序且恰好覆盖 reconciled
revision watermark 内的每条 canonical revision。未达 watermark 的新 revision 可以已有 write-through
Queued entry，但若某 revision 已在 coverage 内却无 entry，manifest 无效。

双槽 pointer bytes 的唯一 schema 为：

```json
{
  "schema": "plico.projection.root-pointer/v1",
  "root_hash": "64-char-lowercase-sha256"
}
```

Pointer 必须是 JCS bytes，root hash 必须精确解析、验 hash、验全量 root chain 和重放 CurrentView 后
才能发布给上层。Active/candidate 不接受路径、generation 或可选 fallback 字段。

Hash domain 分开冻结：

```text
plico.projection.manifest-segment.v1\0
plico.projection.manifest-root.v1\0
plico.projection.current-view.v1\0
plico.projection.builder-spec.v1\0
plico.projection.embedding-artifact.v1\0
```

### 3.4 EmbeddingArtifact

P3-A 不自制 binary codec。Artifact 使用 JCS canonical envelope：

```json
{
  "schema": "plico.projection.embedding-artifact/v1",
  "projection_id": "uuid",
  "source_revision_id": "uuid",
  "source_content_hash": "sha256",
  "builder_spec_hash": "sha256",
  "dimension": 384,
  "encoding": "f32-json/v1",
  "vector": []
}
```

Vector 长度必须等于 dimension，每个值必须是 finite f32 且至少一项非零。Artifact hash 是上述 JCS
bytes 加独立 domain 的 SHA-256。`ArtifactDescriptor` 精确为：

```json
{
  "artifact_hash": "64-char-lowercase-sha256",
  "byte_length": 4096,
  "artifact_schema": "plico.projection.embedding-artifact/v1",
  "dimension": 384,
  "source_revision_id": "uuid",
  "source_content_hash": "sha256",
  "builder_spec_hash": "sha256"
}
```

Ready 发布前和每次 restore 时均交叉验证 descriptor、artifact envelope、文件名/hash、source 和
active spec。`l2_after_matryoshka_truncation_v1` 的持久向量必须满足
`abs(l2_norm(vector)-1.0) <= 1e-4`；`provider_native` 不额外声称归一化。Artifact dimension 必须等于
BuilderSpec 的 effective `dimension`，不能用 `raw_dimension` 冒充。

所有 JSON u64 字段必须 `<= 2^53-1`。Builder dimension 上限为 65536；单 artifact 文件上限 8 MiB；
active/candidate pointer 上限 4 KiB；segment/root/current-view immutable object 上限 16 MiB。CAS 必须在
同一已打开 fd 上验证 regular、0600 和 metadata size 后进行有界读取；Unix 使用 `NOFOLLOW|NONBLOCK`，
FIFO/device/symlink 不得阻塞或被跟随。自报 `byte_length` 不能代替实际文件上限。

## 4. CAS、锁与原子提交边界

### 4.1 一个 personal vault 只有一个 runtime lock

历史实现若为每个 immutable namespace 分别获取 parent-level exclusive vault lock，会导致第二个
projection ledger 无法安全共存；该风险已由 `PersonalVaultStorage` 的唯一生命周期锁和 sealed
projection bundle 前置收敛。P3-A 的存储边界为：

```text
CAS PersonalVaultStorage（持有唯一生命周期 exclusive lock）
  ├── memory-ledger namespace
  ├── projection-store（manifest + artifacts 的共同生命周期）
  └── object CAS namespace
```

Kernel 只打开一次 `PersonalVaultStorage`，再取得固定枚举的 namespace handle。禁止任意路径 namespace、
第二把锁、kernel/memory/bin 直接访问宿主文件系统，所有 mkdir/read/write/fsync/exchange 都在 CAS
模块内完成。目录为 0700、文件为 0600，拒绝 symlink/special；Linux 不支持原子 exchange 时
fail closed，不提供非原子 fallback。

物理布局冻结为：

```text
<vault>/memory-ledger/{objects,roots/{active,candidate}}
<vault>/projection-store/manifest/{objects,roots/{active,candidate}}
<vault>/projection-store/artifacts/objects/<artifact-hash>
```

`projection-store` 是不可拆分的派生数据单元。fresh bootstrap 先在 vault 内 0700 staging container
构造并完整 replay 一个 clean Genesis pair，写入 0600 JCS/domain-separated seal，随后以整个
`projection-store` 的 `NOREPLACE` rename 发布并 fsync vault parent；运行时不存在直接在 live 路径逐层
创建 pair 的 writer。旧 sibling `projection-manifest`/`projection-artifacts` 属于
`UnsupportedFormat`，不提供 reader、adapter 或自动 reset。

### 4.2 Ready 的提交顺序

Artifact 与 manifest root 不需要也不能伪装成跨目录单个 filesystem transaction。唯一正确边界是
“artifact durable before manifest visibility”：

1. worker 按 manifest Building state 重新加载并验证 exact canonical revision、content hash、active
   builder spec 与 attempt ID；
2. CAS 在 `projection-store/artifacts` 内写临时 0600 文件，fsync 文件，`persist_noclobber`，fsync 目录；
3. CAS 回读并验证 artifact domain hash、JCS、dimension 和 source/spec binding；
4. 构造 Ready record、segment、CurrentView 和 Root；每个 immutable object 先 durable 写入并 fsync；
5. 写 candidate pointer、fsync，使用固定双槽 `RENAME_EXCHANGE` 发布 active pointer，再 fsync roots
   directory；
6. 只有第 5 步的 root 可见后，状态才是 Ready。

若 crash 发生在 artifact durable 之后、root publish 之前，只产生不可见 orphan artifact；它永不被
召回，可由后续 owner maintenance 按“未被任何有效 manifest root 引用”删除。若 exchange 后 fsync
结果不确定，writer 进入 poisoned/indeterminate，禁止自动重试，重启后按实际 active pointer 重放。

Canonical commit 与 projection enqueue 故意不是同一事务：canonical 先 durable ack；projection append
失败不撤销 canonical。此时 status 为 `unreconciled`，全量 reconciler 从 canonical watermark 恢复，
不得把缺 record 推断成 Queued 或 AbsentByPolicy。

## 5. 协调、失效与恢复

### 5.1 Reconciler

Reconciler 只以 verified canonical root/view 和 manifest 为输入：

- 为 watermark 内每条 canonical revision 建立恰好一个分类；
- 为 eligible active revision 保证处于 Queued/Building/Ready/Failed/Stale 之一；
- 将 superseded、deleted 或不符合 kind policy 的 revision 转为 AbsentByPolicy；
- 回收过期 Building lease，推进到期 retry；
- 最后以一个 root commit 推进 `reconciled_source`。

内存 mpsc queue 只负责唤醒 worker。进程重启、queue overflow 或通知丢失不能改变 durable state；worker
始终从 manifest 扫描 Queued/expired state。

启动只在四项证据同时成立时自动激活：本生命周期持 vault lease 后真实创建目录、canonical 是 exact
genesis-only、projection 是 Absent、provider identity 已验证且在首个 projection write 紧前再次 revalidate。
任何既有 vault 的 Absent、GenesisOnly、BuilderMismatch、ResetRequired、Prepared/Applied marker、
Unsupported 或 Unavailable 都只报告 typed health，普通 startup 零 projection write。Prepared/Applied 也不
自动 recovery；只有 authenticated `personal-owner` 的显式 `AllEligible` 请求可以 bootstrap/resume/change/
reset/recover。`CurrentRevision` 只适用于健康且 builder exact 的现有 controller。

进程内只有一个 `ProjectionRuntime` 持 projection controller 与 worker lifecycle。worker 即使 startup 暂时
Unavailable 也创建为 idle；owner cutover 安装 Ready controller 后用 content-free wake 激活，无需重启。
worker 每个 job 持一次 lifecycle read guard，provider 调用期间禁止 owner 换代，job 间检查 owner-pending
并让出；owner 在同一 owner gate 下取得 lifecycle write guard，任何 mutation 前重验 sealed provider。
worker 的 reconcile/claim/complete 若遇到 writer poison 或 commit indeterminate，runtime 保留 controller
capability、进入 `worker_restart_required`，停止自动写；provider identity drift 则保持 control plane 可读，
worker 进入 `provider_changed_restart_required` 且后续 wake/tick 零写。daemon shutdown 先设置 stopping，
拒绝新的 owner/start/notify，再 stop+join worker，最后通过 owner gate 与 lifecycle write barrier；shutdown
返回后 projection 不得再写。

readiness 从同一个无 provider I/O 的 runtime snapshot 产生，分别报告 projection `control_plane` 与
`worker` 及各自 stable reason。identity drift 时 overall canonical/read/lexical readiness 仍为 true，
control plane 为 ready，worker 与 embedding provider 为 unavailable；shutdown 期间 overall readiness 为
false，worker reason 为 `runtime_shutting_down`。线程存在性不得伪装成 worker readiness。

### 5.2 失效

- model/dimension/builder spec 变化：在一次 manifest generation 内激活新 spec 并将旧 Ready 标 Stale；
- canonical update：新 revision Queued，旧 revision AbsentByPolicy；
- canonical delete：tombstone revision 和旧 active revision均 AbsentByPolicy；
- artifact 缺失、hash 不符、shape/权限/文件类型非法：manifest 验证成功后进入 `repair_required`，普通
  status、root 读取和 commit 均 fail closed；唯一 repair 临界区先 durable 追加 Stale invalidation，
  再处理 derived object。若 Stale publish 失败或结果 indeterminate，绝不删除 artifact，也绝不返回
  Ready；
- `ArtifactMissing` 无对象可清理。`ArtifactHashMismatch`/`ArtifactInvalid` 在 Stale 已 durable 后，由 CAS
  以同 vault lock 下取得的 `NOFOLLOW|NONBLOCK` same-fd identity/content snapshot 再验证；只有当前 view
  仍是对应 Stale 且对象仍是同一坏对象时才 unlink+fsync。cleanup 失败保持 Stale 并报告稳定 maintenance
  required；重启后同一个 repair 入口必须重试 cleanup。repair、cleanup、artifact put 与 manifest commit
  共用唯一 writer 临界区，迟到 cleanup 不得删除同 spec 重建出的新 Ready artifact；
- manifest root/segment/current-view 损坏：canonical runtime 仍可服务 get/lexical，但 projection
  subsystem fail closed，需 owner 显式 rebuild；不能静默清空并谎报成功。

### 5.3 全量重建

Projection store 本身也是派生数据，但 owner reset 禁止先删 live tree、原地递归清空或分别替换
manifest/artifacts。只有持有 scoped personal-owner canonical proof，且在唯一 projection lifecycle claim
内再次重放并得到当前 schema 的 `ResetRequired`（ManifestIncomplete、ManifestIntegrityInvalid、
StorageLayoutInvalid 或 CanonicalLineageInvalid），才允许破坏式 reset。future/unknown schema 为
`UnsupportedFormat`，真实权限/I/O/资源故障为 `Unavailable`，两者均零写且禁止 reset。

Reset 在同一 filesystem 的 0700 staging container 中构造完整 clean Genesis pair，经 full replay、zero
artifact inventory、exact tree seal 后写入固定 0600 JCS marker。marker 是两相状态机：
`Prepared` 绑定 typed reason、旧 live identity/fingerprint、target seal/tree/active evidence 和固定 transition
evidence name；整个 `projection-store` 只通过一次 `RENAME_EXCHANGE` 切换。双 parent fsync、live identity、
tree 与 active evidence 验证完成后，使用 fixed transition evidence 将 marker 推进为
`AppliedMaintenance`。`Prepared` 对外是 `ResetPending`；`AppliedMaintenance` 是
`ProjectionMaintenanceRequired`；marker current-schema 损坏或不可能拓扑为 `ManualIntervention`。任一
post-mutation durability 不确定为 `CommitIndeterminate` 并 poison 当前 claims，禁止同生命周期重试。

EXCHANGE 后的旧 pair 永远先留在 marker 绑定的 0700 quarantine。owner recovery 仅按 marker/seal/identity
真值，以 fd-relative `NOFOLLOW|NO_XDEV` 的有界清理器续办；绝不 `remove_dir_all` 不可信旧树。清理上限为
4096 entries、depth 8；private regular 仅在单文件 `<=16 MiB` 时读取并哈希，oversize/non-private/special
只绑定 metadata evidence。symlink/FIFO/socket/device leaf 只 unlink 自身；目录只在 `openat2`
`BENEATH|NO_SYMLINKS|NO_XDEV` 成功后遍历，已打开的 0755 目录可 `fchmod(0700)`，无法安全打开的 000
目录要求 manual intervention。pair、seal、container 和 marker 的每一步 unlink 后均 fsync parent，且恢复
接受协议定义的单调 partial states；pair 存在而 seal 丢失等不可能状态必须保留 marker 并 fail closed。

quarantine 与 Applied marker 全部清除并 durable 后，store 才重新成为 clean GenesisOnly；随后 owner 显式
`AllEligible` resume 激活 sealed builder、全量 reconcile，并返回实际 durable generation/event/source
watermark。健康 GenesisOnly 从不 whole-exchange：只在原 pair 内有界清理未引用 manifest/artifact orphan，
保留 genesis objects/history 后再 activate。重建前后 canonical 整棵目录树（路径、类型、mode、mtime、
atime、bytes digest）必须完全相等。

故障门禁覆盖 Prepared marker durable 但 pair 未交换、pair 已交换、Applied transition durable、marker
exchange、quarantine pair/seal/container 逐步删除，以及 active marker unlink 后 parent fsync 失败。所有
cutpoint 都必须 drop/reopen 后按 live/marker/quarantine 拓扑恢复，不使用 mtime 猜测，不重复收费操作，
也不接触 canonical filesystem。

固定 deterministic builder fixture 中，重建必须得到相同 eligible/Ready/Absent 数量、相同 source
watermark 和逐 artifact hash。真实远程 provider 若不能保证逐字节确定性，仍必须保证 source/spec
绑定、数量、shape 与 recall-independent 状态正确；不得用不稳定 provider 跳过 deterministic gate。

## 6. Personal-owner 与公共协议边界

### 6.1 权限

- 所有 canonical 和 projection 都属于一个自然人的 PersonalVault；schema 不出现 tenant、organization、
  workspace、billing 或共同所有权；
- `projection.status` 先通过 canonical 当前 policy 鉴权。未授权 revision 与不存在 revision 都返回同一
  NotFound，避免枚举；public status 只接受当前 active revision，不借 projection 接口开放 history；
- artifact locator/raw vector 不对外返回；
- `projection.rebuild` 只允许 trusted context 解析出的 `personal-owner`，普通 readable/writable role
  无权触发模型费用或全量维护；
- internal `projection-worker` 不能由 bearer 或 payload 申请，也不获得新的 canonical mutation 权限。

### 6.2 `plico.personal.v2` 一次性切换

六态不能诚实映射回 V1-A 的 `NotRequested/Pending/Ready`：Failed 不是 Pending，Stale 不是 Ready，
Building 也不能从 `embedding: None` 推断。P3-A 完成时进行一次破坏式协议切换：

- protocol 从 `plico.personal.v1` 改为 `plico.personal.v2`；
- 原 13 项中 12 项保持操作名；
- 删除 `memory.index_status` 和 MemoryEntryView 中的 `embedding_state`；
- 新增 `projection.status` 和 owner-only `projection.rebuild`；精确 catalog 为 14 项；
- 同一变更更新 TCP/UDS/Embedded、RemoteClient、MCP、aicli、plico-agents 和 benchmark；
- 物理删除 v1 reader、enum、alias、translation test 与 fallback，不双轨运行。

`projection.status` 输入只接受 `kind=memory_embedding` 与当前 active revision ID。响应先报告 observation：

```text
observed     -> 必须带六态之一
unreconciled -> canonical 已存在，但 manifest record/coverage 尚未建立
unavailable  -> manifest 损坏、不可读或 writer poisoned
```

Observed 响应绑定 revision ID、完整 canonical content hash、manifest event watermark 和 reconciled
canonical watermark。完整 content hash 可以出现在**鉴权成功后的 typed response**，用于调用方验证
identity；不得进入 trace、日志、metric label、错误文本或未鉴权诊断。普通响应不暴露 attempt ID、
lease token、artifact locator、raw vector、provider URL/错误或宿主路径。Failed 只返回稳定 category、
retryable、attempt 和 retry-not-before；Stale 只返回稳定 reason。

`projection.rebuild` 输入必须显式选择一个当前 active revision 或全部当前 eligible revision。成功响应
只承诺 Queued transition 和 manifest root/watermark 已 durable，不承诺 build 已完成，客户端不得
自动重试写操作。
外层 projection 输入字段唯一为 `kind`，selector discriminator 唯一为 `type`，不接受 alias；CLI 同样要求
显式且互斥的 `--revision-id` 或 `--all-eligible`，不得用缺省值暗示全量收费操作。所有 rebuild wire 错误
`retryable=false`，调用方必须按 stable category 决定人工处理或 restart。

Capabilities 在 P3 门禁完成后才能声明：

```text
memory_embedding.control_plane = supported
memory_embedding.retrieval     = unsupported
memory_vector_recall            = unsupported
memory_hybrid_recall            = unsupported
```

`memory.recall` 仍只返回 `matched_by=lexical_overlap`。可以增加真实 canonical/projection watermark 和
degradation，但 Ready embedding 未参与检索时不得出现在 `matched_by` 或 score components。

## 7. 一次性切换顺序

1. **冻结与 preflight**：验证 canonical root chain、逐 revision content hash、当前 provider 的 stable
   builder identity；记录 canonical baseline count/hash，不读取 runtime vector 作为迁移来源。
2. **CAS 存储重构**：一次打开 PersonalVaultStorage 和固定 namespace handles；所有现有 canonical
   门禁先保持通过。
3. **实现但不双写**：完成 manifest replay/validator、artifact store、builder、reconciler 和 fault
   injection；旧生产路径仍是唯一 active 路径，不能 shadow-write manifest。
4. **离线/单版本 cutover**：停 daemon、持有同一 vault lock，从 canonical 建 projection genesis 与
   全量分类；eligible revision 全部 Queued。不存在“迁移 canonical 内嵌向量”步骤。
5. **同一代码变更切调用点**：builder/reconcile/persist/restore/status 全部只用 manifest；删除
   `MemoryEntry.embedding`、三态、旧 retry truth、LongTerm write-time dedup 和 memory vector readers。
6. **全量 rebuild**：在固定 builder 下把 eligible revision 构建为 Ready，验证 artifact 双射、
   watermark、数量/hash；provider failure fixture 必须保留真实 Failed/Queued，而非伪 Ready。
7. **公共协议切换**：所有 transport/client/demo 从 personal.v1 一次切到 personal.v2，v1 请求无状态
   变化地拒绝。
8. **故障与重启门禁**：分别注入 Queued、Building、artifact durable、root exchange、post-exchange
   fsync、model change、missing artifact、corrupt manifest；证明协调结果和 canonical 不变。
9. **发布**：全部门禁与真实 UDS dogfood 通过后，才把本 ADR 改为 Accepted、计划阶段改为 Completed
   并在 capabilities 声明 control plane supported。

## 8. 验收门禁

- manifest event/root/view raw JCS、domain hash、sequence、previous link、source binding 和 replay 全验证；
- watermark 覆盖内每条 canonical revision 与 manifest entry 恰好一一对应，无遗漏/重复；
- 删除 projection store 后可从 canonical 全量重建，canonical bytes/hash/root chain 完全不变；
- deterministic builder 下 rebuild 前后 eligible count、Ready count、artifact hash 集合和 watermark 相等；
- model/dimension/spec 变化使全部不匹配 Ready 在一个 root generation 内变为 Stale，旧 artifact 永不
  servable；
- crash 在 Queued/Building/artifact write/root publish 任一点均可重启协调，只有 durable root 决定状态；
- enqueue 失败时 canonical write 仍成功且 status 为 unreconciled；manifest 损坏时 lexical recall 可用；
- role/owner ACL 在 restart 后不变，未授权 status 不泄露 revision 是否存在，普通 role 不能 rebuild；
- personal.v2 catalog 精确 14 项，v1/`memory.index_status`/旧 embedding enum 全部无生产定义与调用方；
- trace 能按 request/run ID 复原 validate→canonical load→manifest transition→artifact verify→root publish，
  但 canary 证明正文、query、tag、bearer、完整 content/root/artifact hash、provider body 和 host path 未泄露；
- projection reset 的 `inspection`、`prepared`、`pair_exchange`、`marker_transition`、`quarantine_cleanup`、
  `seal_cleanup`、`container_cleanup`、`marker_clear`、`recovery` 与 `complete` 必须由同一个非 secret、
  canonical UUID `reset_operation_id` 关联；字段只允许稳定 phase/outcome/result category/reset reason、计数与
  durable watermark，禁止记录 vault/staging/transition basename、路径、seal/tree/active digest、canonical/content/
  artifact/builder hash、正文、provider/model、role 或 bearer；
- public recall 仍只报告 lexical_overlap；没有 benchmark/ADR 时 vector/hybrid 不得因 artifact Ready 被激活。

## 9. 明确 unsupported 与非目标

P3-A 不包含：

- Memory vector/hybrid recall 的生产激活、ANN 或检索排序变化；
- Memory BM25、KG、summary、Wiki、claim 或 evidence projection；
- Object HNSW/BM25 的 manifest 迁移或行为改变；
- Hot/Warm/Cold/Dormant、temperature、deep recall、rehydrate、腐败策略或字段占位；
- 文档、表格、PPT、GUI、export/absorb 人类投影；
- hard erase、history/restore、TTL/importance/access/heat 状态迁移；
- 企业 tenant、组织 workspace、多所有者、组织 RBAC、跨 vault projection；
- v1 兼容 reader、协议 alias、状态降级映射、长期 shadow write 或双轨 store。

ADR-0002 把 retrieval temperature 列入最终 ProjectionManifest 目标模型，但 P3-A 明确不为尚未通过
DECAY gate 的 P4 能力预留字段。若 P4 被实验接受，必须通过新的 schema version/ADR 增加正交的
thermal event/control plane，而不是把空 temperature 字段提前写入 v1 manifest。

## 后果

正面后果：embedding 状态跨重启可验证；模型变化和失败不再被三态掩盖；projection 可从 canonical
完整重建；public status 能区分未协调、失败、过期和不适用；检索质量决策与存储正确性解耦。

成本：需要重构 vault lock 所有权、建立第二个 append-only 派生日志、一次公共协议 major 切换，并
删除 LongTerm 当前 silent dedup 语义。Object projection 仍是独立后续债务，P3-A 不能宣称已经获得
vault-global projection coverage。

## 拒绝的替代方案

- **在 `MemoryEntry.embedding` 旁双写 manifest**：保留两个真值，crash 后无法裁决；
- **把所有 `None` 映射为 Queued**：会把未协调、AbsentByPolicy 和未来 thermal absence 混为一谈；
- **直接复用 personal.v1 三态**：Failed/Stale/Building 无真值表达，属于兼容壳；
- **Ready 后顺便启用 vector recall**：把控制面正确性与召回质量、score fusion 和退化策略耦合；
- **为 P4 预留 temperature 空字段**：把 research 候选伪装成已冻结能力；
- **另开一个 `ImmutableLedgerStorage`**：与同 vault 生命周期锁冲突，或迫使出现不受同锁保护的写者；
- **将 artifact 写入普通 AIObject 并复制 legacy tenant/scope metadata**：把派生工件混入用户对象域并
  延续已删除的命名空间语义；
- **兼容迁移 runtime 向量**：这些向量没有 durable source/spec/attempt 证据，不能升级为 Ready。
