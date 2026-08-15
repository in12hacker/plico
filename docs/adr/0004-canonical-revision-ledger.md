# ADR-0004：Canonical Revision Ledger 与 Truth Firewall

- 状态：Accepted / Completed（V1-B canonical ledger 与受支持旧 vault 迁移已完成）
- 日期：2026-08-13
- 完成审计：2026-08-14
- 上位决策：[ADR-0001：个人数字分身与记忆原生数据模型](./0001-personal-digital-twin.md)
- 数据分层：[ADR-0002：Canonical Memory、按需投影与可逆检索腐败](./0002-canonical-memory-and-reversible-retrieval-decay.md)
- 公共契约：[ADR-0003：个人数字分身统一领域模型与公共能力契约](./0003-personal-twin-domain-and-public-capability-contract.md)
- 实施计划：[个人数字分身公共能力演进计划](../plans/personal-twin-public-capability-evolution.md)

## 决策摘要

V1-B 在 projection manifest、Thermal Recall、LLM Wiki 编译投影和 evidence/claim 之前，先建立唯一的 canonical revision ledger。稳定的 `memory_id` 表示一条记忆在多次纠错中的逻辑身份；不可变的 `revision_id` 表示一次具体内容提交；`parent_revision_id` 只表达同一 `memory_id` 内的直接版本前驱；内容和 revision 均由稳定 SHA-256 摘要校验。

create、update、supersede 和普通 delete 都只追加 canonical record，不原地覆盖或清除历史。当前视图、embedding、BM25、KG、摘要、Wiki、热度、访问计数和 worker 状态都不是 canonical truth。普通 delete 追加 tombstone；物理擦除属于独立的 owner-only retention workflow，在其完整门禁完成前保持 unsupported。

这项决策不增加企业 tenant、organization 或跨用户共享模型。所有记忆仍属于一个个人 vault；`AgentRole` 只是该数字分身内部由可信 transport context 绑定的执行职责。

V1-B 已按本 ADR 落地唯一 append-only ledger、expected-head、current-view policy、离线迁移与原子切换；旧 snapshot writer、原地 supersession、破坏性维护入口和运行时兼容读路径已物理删除。公共协议继续把当前 revision ID 命名为 `entry_id`，不提前公开仍未实现的 history、restore、hard erase 或 projection control operation。

完成声明严格限定在 Truth Firewall：Projection Manifest、projection 完成态、Thermal Recall、deep recall、evidence/claim、公开稳定 `memory_id`、history/restore/hard erase 和 legacy causal migration 均不在 V1-B 范围内，继续 typed unsupported 或 fail-closed。

历史 V1-B release evidence 由独立的完整 release artifact 保存，不随源码仓库提交本机 trace、binary 或不完整 bundle。当时的单次 run 只证明正确性、故障语义和本机观测值，不构成性能优越性结论。

## 背景与问题

V1-A 已经形成真实的个人记忆闭环：Working Memory 先持久化再发布，update 创建新 entry，delete 软删除，late embedding task 不能复活非 active entry。但当前模型还不能作为后续演化的稳定真值底座：

1. `MemoryEntry::id` 实际是 revision 级 ID，没有跨纠错稳定的 logical identity；
2. update 通过修改旧 entry 的 `superseded_by` 和新增 entry 的 `supersedes` 表达版本关系，delete 通过修改 `deleted_at` 表达状态，逻辑上仍存在原地改写；
3. 持久化 snapshot、恢复、compact 和直接 store/delete 路径可能绕开同一个 revision commit 不变量；
4. `embedding` 与访问统计仍和 canonical entry 同结构存储，后续 manifest 迁移会出现双真值风险；
5. 如果先实现冷热迁移或 Wiki 维护，派生投影将绑定到不稳定身份，历史压缩还可能被误作 canonical 删除。

因此 V1-B 不是功能扩张，而是一道 Truth Firewall：任何派生系统只能读取已经提交的 canonical revision，任何维护任务都不能静默改写 canonical history。

## 决策

### 1. 个人 vault 是唯一所有权边界

- 一个部署 root 只承载一个自然人的 personal vault；V1-B 不引入 tenant table、organization、team、workspace 或组织级 RBAC。
- `memory_id` 在该 personal vault 内唯一，不含 tenant 前缀，也不编码 AgentRole。
- `role_id` 来自 UDS/Embedded 的可信 local context 或 TCP credential 解析结果，只记录“谁代表个人执行了提交”，不建立第二个数据所有者。
- 现有 `tenant_id` 仅可作为离线迁移的旧 namespace 输入；它不进入新 ledger identity，不出现在新的 public schema，也不得被改名包装成组织级 tenant。
- 不同 AgentRole 的可见性是同一人的最小权限策略。role policy 可以限制当前视图，但不能让同一 revision 在多个 owner 真值之间复制或分叉。

### 2. 稳定身份与不可变 revision

V1-B 的 canonical 最小模型为：

```text
MemoryRevision
  memory_id: UUID
  revision_id: UUID                   # 一次不可变提交
  parent_revision_id: UUID?           # 仅同一 stream 的直接前驱
  content: MemoryContent
  canonical_content_hash: SHA-256
  tags: [String]
  memory_type: MemoryType
  cognitive_tier: MemoryTier
  deleted_at: timestamp?              # 仅 append-only tombstone revision 设置
  committed_at: timestamp
  committed_by_role: trusted role id
```

V1-B 不用空字段冒充 P2：evidence link、claim、valid time、user review 和 provenance 在 P2 schema 被接受并实现前，不加入当前 ledger record。

`parent_revision_id` 只有一个含义：同一 `memory_id` 中此次 revision 的直接内容前驱。它不表达语义相似、跨记忆矛盾、evidence 引用或“模型认为这段内容过时”。跨 stream 的 claim supersession 必须等 P2 reconciliation 设计，不能复用版本父指针。

每个 stream 在任一 canonical watermark 上至多有一个 active head。update 使用 expected-head compare-and-commit；并发请求引用过期 head 时返回 typed conflict，不能静默产生分叉或靠“最后写入者获胜”覆盖。

### 3. 内容哈希与 revision 哈希

`canonical_content_hash` 只覆盖 `MemoryContent`，不覆盖 revision identity 或运行状态。各 variant 使用带版本的 domain separator、variant tag 和无歧义长度前缀；其中 `Structured(serde_json::Value)` 固定使用 RFC 8785 JCS：

```text
SHA-256("plico.memory.content.v1\0" || variant-tag || canonical-payload)
```

Text/ObjectRef 使用原始 UTF-8 bytes；Procedure/Knowledge 使用字段标签、长度和 IEEE-754 bit pattern 的版本化 typed encoding；Structured 必须使用维护中的 RFC 8785/JCS 库及固定 golden vectors，禁止普通 JSON serialization、手写 key 排序、数字格式化或字符串 escape。若候选库与当前 JSON 语义存在差异，实施暂停并回到架构评审；不能在库外增加兼容规范化壳。哈希输入不包含 tags、tier、identity、时间、embedding、projection state、access count、worker error、thermal state或其他运行数据。完整 revision record 的静默篡改由其 CAS CID/持久化 manifest digest 校验，而不是另造第二套 revision hash。

完整内容哈希和 revision 哈希默认不写入 debug 日志，也不进入 public response，避免对低熵个人内容提供离线猜测信号。校验结果只记录 `hash_verified` 与稳定失败类别。

### 4. Canonical、runtime 与 projection 字段归属

| 归属 | 字段/记录 | 约束 |
|---|---|---|
| Canonical identity | `memory_id`、`revision_id`、`parent_revision_id` | 不可由索引或当前视图重建后重新编号 |
| Canonical payload | `content`、`canonical_content_hash`、`tags`、`memory_type`、`cognitive_tier` | 任一语义变化都创建新 revision |
| Canonical audit | commit time、trusted role、revision parent、tombstone revision | 只追加；重启后顺序和含义不变 |
| Runtime | queue attempt、last error category、worker timing、cache state、候选/exposed/selected/used counters | 可丢弃或按独立 retention 压缩；不能改变当前事实 |
| Projection | embedding/ANN、BM25、KG、summary、Wiki、reranker artifact、projection watermark | 可删除可重建；V1-B 后由唯一 manifest 接管 |
| Thermal | Hot/Warm/Cold/Dormant、residency、rehydrate state、衰减分数 | 只影响投影成本，绝不进入 revision hash |
| Policy | user pin、retention obligation、显式保留/擦除授权 | 作为独立策略事件；不能复用 access count 或 temperature |

`importance` 必须按来源拆开：用户明确声明的长期保留意图属于 policy event；模型计算或启发式 importance 属于可重算 runtime/projection signal。迁移器不得把当前混合字段静默解释成用户授权。

### 5. Mutation 的追加语义

所有 canonical mutation 使用相同阶段：

```text
validate -> load_head -> construct_commit -> persist_ledger
         -> publish_current_view -> enqueue_projection
```

- **create**：追加一个新的 root revision；`memory_id` 在其后续全部 revision 中保持不变，持久化成功后才发布 current view。
- **update/correct**：以当前 head 为 `parent_revision_id` 追加新 content revision。旧 revision 字节不变，不再写 `superseded_by`。
- **supersede**：同 stream 内等同于有明确原因的 head advancement；跨 stream supersession 在 P2 前 unsupported，不能由 semantic dedup 自动执行。
- **delete**：仅当目标是当前 active head 时追加一个新的 tombstone revision；它共享 `memory_id`、以旧 head 为 parent，并设置自身 `deleted_at`，但绝不修改旧 revision。revision 和父链仍可供受控历史/审计恢复；现有 public `memory.delete` 不承诺物理擦除。
- **restore**：未来只能在 tombstone 之后追加显式 restore revision；V1-B 不把它加入现有 13 项公共协议。
- **projection enqueue**：只能发生在 durable ledger commit 之后。enqueue 失败不回滚 canonical；reconciliation 从 ledger watermark 重新发现任务。

持久化失败不得发布新 head或 tombstone；publish 前进程崩溃时，重启必须从 ledger 重建 current view；publish 后 enqueue 前崩溃时，canonical 仍成功且派生任务可协调。不得通过清空 live state 后尝试恢复的方式实现切换；新 ledger 必须完整验证后再原子发布 current-view 指针。

### 6. 当前视图与派生状态

active/deleted/superseded 是对 revision parent chain 的当前视图，不再是可随意改写旧 `MemoryEntry` 的布尔字段。`deleted_at` 只能出现在新追加的 tombstone revision。current view 可以缓存，但必须能由 ledger 重建，并携带它所对应的 canonical watermark。

V1-B 不提前实现 Projection Manifest。迁移完成后，embedding 仍可有短暂的后续单路径迁移窗口，但 canonical ledger record 从第一天起不得包含新 embedding 写入。随后 P3 在一次 cutover 中让 indexing、reconciliation 与 recall 只读写 manifest，并物理删除旧内嵌 embedding 状态源；禁止长期双写。

现有 13-operation public contract 在 V1-B 保持不扩张：`entry_id` 明确映射当前 `revision_id`。在 `memory.history/correct` 及稳定 identity 的 public schema 被单独验收前，不增加 `memory_id` 字段或兼容 alias。

### 7. 普通删除、compact 与物理擦除

普通 `memory.delete` 是可审计 tombstone，不是 hard erase。下列动作一律不得物理删除 canonical revision：

- compact、TTL、未命中衰减、容量回收或层级 eviction；
- embedding/KG/BM25/Wiki/摘要清理；
- supersede、semantic dedup、冲突解决或“较新内容胜出”；
- checkpoint restore、rebuild、daemon restart 或 current-view 修复。

物理擦除是独立的 owner-only retention workflow，在实现前保持 unsupported，不复用 `memory.delete`：

1. 由可信 local-owner context 发起并进行显式确认；
2. 先生成只读 erase manifest，枚举 revision、ledger segment、CAS object、projection、导出物和所有入站引用；
3. 遇到共享 CAS blob、未知引用、不可访问备份或不完整 manifest 时 fail closed，不做部分“成功”；
4. 擦除 canonical 与所有可重建副本后验证不可召回、不可重建、不可由旧 current view 复活；
5. 只保留不含正文、内容哈希、memory/revision ID 的最小随机 receipt 和策略/时间/计数；
6. crash recovery 必须幂等并明确报告 `planned/in_progress/verified/failed`，不能把排队当成完成。

若底层 CAS 去重导致目标字节仍被其他明确保留的 canonical record 引用，系统必须要求用户裁决扩大擦除范围或保留共享对象；不能静默破坏其他记忆，也不能谎报已擦除。

### 8. 一次性迁移，不设兼容壳

V1-B 使用离线、停写、一次性迁移：

1. 备份现有 personal vault，并记录文件数量、记录数量、CID 与整体 manifest digest；
2. 只读验证所有旧 `id/supersedes/superseded_by/deleted_at` 关系，拒绝 cycle、missing parent、cross-role/cross-namespace link、多个 active head 和无法解释的 branch；
3. 旧 `MemoryEntry::id` 原值一对一成为 `revision_id`；每个经验证的线性 revision chain 使用其唯一 root revision ID 作为稳定 `memory_id`，映射写入带摘要的 migration manifest；孤立 entry 的 `memory_id` 等于自身 revision ID。二者类型仍严格区分，不能在调用点混用；
4. 旧 `supersedes` 仅在被完整双向验证时转换为 `parent_revision_id`；不能仅凭时间、文本相似度或数组顺序猜测；
5. 旧 `superseded_by` 转为 parent/head 关系，旧的原地 `deleted_at` 转为新追加的 tombstone revision；所有 content revision 计算 `canonical_content_hash`；
6. 在临时目标中重放 ledger，逐项比较 revision 数量、active/tombstoned head、内容字节、tags、tier、role/namespace 和 hash；
7. 通过后原子切换唯一 ledger/current-view root，再启用写入；
8. 同一切换中重写全部生产调用点并物理删除旧 reader、writer、字段、compact 破坏路径和双写测试。

迁移后不保留旧格式运行时读取器、adapter、alias、fallback 或两套持久化。回滚只允许在新写入发生前整体恢复已校验备份；一旦新 ledger 接受写入，必须前向修复，不能让进程按不同格式随机启动。

### 8.1 旧字段的唯一迁移语义

迁移器必须生成 owner-only、内容寻址且纳入目标 root digest 的不可变 migration manifest。它保存旧记录 CID、摘要和原始字段，只用于审计迁移；新 runtime 不得读取它作为旧格式 fallback。

| 旧字段 | 唯一目标 | 拒绝的错误解释 |
|---|---|---|
| `agent_id` | 经 owner 批准的一次性映射解析为本地 `AgentRole`，用于 access policy；原值作为 `legacy_actor_hint` 留在 manifest | 不进入 identity，不冒充可信 `committed_by_role` |
| `tenant_id` | 只证明整个输入属于一个 personal-vault namespace；原值进入 manifest，切换后从 runtime 消失 | 不进入 ledger、policy、current view 或 public schema |
| `scope` | 独立 append-only access policy event | 不进入 revision payload/hash，不保留运行时 `MemoryScope` 兼容枚举 |
| `causal_parent` | 独立 canonical `CausedBy` relation assertion | 不复用 `parent_revision_id`、KG edge、tag 或 projection |
| `created_at` | 原值作为 `legacy_created_at` 进入 manifest | 不冒充 durable `committed_at` |
| `importance` | 原值进入 manifest；新 runtime 初值为 `Uncomputed` | 不生成 pin、retention、temperature 或 ranking 权重 |
| `ttl_ms/original_ttl_ms` | 原值进入 manifest；停止旧 TTL 执行语义 | 不生成 tombstone、hard erase、隐藏或 thermal 状态 |

迁移 revision 的 `committed_by_role` 是实际授权迁移的 credential-bound local-owner/maintenance role，`committed_at` 是写入新 ledger 的真实 import commit time。ledger sequence 只由已经验证的 parent chain 决定，不按旧时间猜测。

### 8.2 Group 与个人访问策略

`MemoryScope::Group(group_id)` 只允许迁移为同一个人 vault 内的 `ExplicitRoleSet` policy，不能默认降为 Private、扩大为 Shared，也不能引入 organization/team/group runtime。迁移输入必须包含 owner 授权并纳入 manifest digest 的精确映射：

```text
legacy group bytes -> credential-bound local AgentRole ID set
```

迁移后 readers 为 local-owner、来源 role 和映射 roles；writers 为 local-owner 与来源 role。group 名只保留于 manifest，未来新增 role 不自动获得访问权。缺映射、未知 role、冲突映射、空白或非法 group 一律 `unresolved_group_audience` 并拒绝发布目标 root。

Private 迁移为来源 role 与 local-owner 可读写；Shared 迁移为 cutoff 时 personal vault 内全部 active credential-bound roles 与 local-owner 可读、来源 role 与 local-owner 可写，未来新增 role 不自动获得访问权。同一 stream 历史 scope 变化必须按 revision 顺序生成 policy events，不能只迁移 head。

一个迁移批次只接受一个非空 legacy namespace，或全部记录均来自可证明的 pre-tenant schema。多个值、显式空值、缺失与显式值混杂均拒绝。current view 只缓存 active head、policy watermark 和解析后的访问集合，并可完全从 ledger 与 policy log 重建。

### 8.3 旧因果关系的无损保留

`child.causal_parent = parent_entry_id` 转换为：

```text
subject_revision_id = child.revision_id
predicate           = CausedBy
object_revision_id  = parent.revision_id
epistemic_state     = ImportedUnverified
provenance          = migration_manifest_id
```

该 relation 不参与 V1-B ranking、KG 推理或 public response；只有调用 role 同时可读两端时 current view 才可暴露。未来 P2 review 只能追加审阅结论，不能改写导入关系。目标缺失或不唯一、self-link、因果 cycle、跨 personal-vault namespace 一律拒绝；同一 vault 内跨本地 role 可以保留。canonical relation schema 未落地前，任何含 `causal_parent` 的 vault 都不得迁移。

### 8.4 离线迁移发布协议

迁移器必须是独立 offline-only binary/crate；legacy DTO 只存在于该工具，不能链接进 library、daemon 或 public client。流程固定为：

1. 在 vault 父目录取得独占锁，确认 daemon、demo 和 workers 全部停写；
2. `inspect/dry-run` 只读验证唯一 legacy schema、所有索引引用、CAS CID/bytes、entry count、JCS/hash 与全局 revision/relation 图；
3. 在同父目录、同文件系统构造完整 staging root，写 immutable ledger、policy/relation logs、current-view watermark 及 source/migration manifests；迁移凭据只以 active role/expiry cutoff 的 domain hash 持久化，不保存 bearer/token hash；
4. Memory 在交换前完成一次完整 typed replay 并生成只有 typed builder 能构造的 seal；CAS 校验 source/staging tree 双射、所有文件和目录 digest 及 `fsync` 后，使用维护库提供的 `renameat2(RENAME_EXCHANGE)` 原子交换，禁止手写 syscall；
5. 保持同一 vault 独占锁，CAS 在交换后按 seal 复核完整 tree 与 active/root object bytes；失败立即 `RENAME_EXCHANGE` 回滚。验证通过后先在 owner-only staging parent 内将旧 source 根与全部目录收紧为 0700、全部普通文件收紧为 0600，再原子重命名为 rollback backup，避免 permissive legacy root 暴露出 rename-to-chmod 窗口；随后写入 0600、fsync、隐去 credential bytes/hash 的 backup evidence。任何后续失败都必须先按预扫描 manifest 恢复原权限与 bytes 再整体换回。尚无新写时可整体恢复已校验 backup，一旦新 ledger 接受写入只允许前向修复。

cycle、orphan、单边 supersession、branch、重复 ID、多 head、cross-role/namespace revision parent、deleted non-head、未知 enum/schema、Group 未解析、CID 缺失/损坏、JCS unsafe number、symlink/special file、source 在 dry-run 后改变、空间不足、跨文件系统或不支持 atomic exchange均 fail closed。不得用时间、相似度、数组顺序或 embedding 猜测。

### 9. 结构化 tracing

每个 mutation 建立一个结构化 span，至少包含：

- `request_id`、`operation`、可信 `role_kind`；
- `memory_id`、`revision_id`、可选 `parent_revision_id`；
- `phase`：`validate/load_head/construct_commit/persist_ledger/publish_current_view/enqueue_projection`；
- `ledger_sequence`、`canonical_bytes`、`hash_verified`；
- `result_category`、`retryable`、`elapsed_ms`。

compare-and-commit conflict、持久化失败、publish recovery 和 late projection 必须使用稳定类别。日志禁止正文、tags、完整 query、bearer、完整内容/revision hash、provider 原始响应和宿主私有路径。测试断言状态和持久化结果；tracing 用于还原数据流与逻辑流，不能成为成功判定的替代品。

## V1-B 硬门禁

V1-B 只有全部满足时才能标记 Completed：

1. **Identity**：create 后 `memory_id` 稳定；连续 update 只新增 revision；每个 parent 都属于同一 stream，active head 唯一。
2. **Immutability**：update/delete/supersede 前后所有旧 revision 的 bytes 与 `canonical_content_hash` 不变；完整 record 的 CAS CID/manifest digest 仍可验证。
3. **Hash**：RFC 8785 golden vectors、CJK、Unicode normalization 边界、structured content 和重启 round-trip 全部一致；仓库无手写 JSON 规范化实现。
4. **Durability**：persist 失败不发布；在 persist/publish/enqueue 每个故障点重启后 current view 与 ledger watermark 一致。
5. **Concurrency**：两个并发 expected-head update 只能有一个提交，另一个返回 typed conflict；重试不能静默创建重复 head。
6. **Deletion**：普通 delete 只追加一个幂等 tombstone；compact、TTL、rebuild、checkpoint restore 和 late task 都不能删除或复活 revision。
7. **Migration**：valid linear、isolated、deleted fixture 可无损迁移；cycle、orphan、branch、cross-boundary fixture fail closed；迁移前后数量和 hash manifest 相等。
8. **Single path**：生产代码中旧 snapshot writer、`supersedes/superseded_by` mutation、对既有 revision 的 `deleted_at` mutation、破坏性 compact 和运行时 old-format reader 为零；没有 compatibility adapter 或 dual write。
9. **Projection isolation**：删除全部 embedding/索引状态后仍能恢复相同 canonical ledger 与 current view；projection failure 不改变 canonical acknowledgement。
10. **Public truth**：现有 13 项 operation、TCP/UDS/MCP/aicli parity、read-after-write、typed error 和 personal-owner auth 继续通过；history/restore/hard erase/thermal 仍明确 unsupported。
11. **Privacy/trace**：代表性 success、conflict、persistence failure、restart 和 migration rejection trace 可关联完整阶段，且不含正文、凭据、完整 hash 或私有路径。
12. **Quality**：`cargo fmt --all -- --check`、全量 `cargo test`、`cargo clippy -- -D warnings`、三个保留 binary build 与真实 UDS dogfood 全部通过；benchmark artifact 带 run manifest、dataset hash 和 failure ledger，失败 fail closed。

性能只与 V1-A 基线和同机重复噪声比较。若 append-only commit 的 p95/p99、写放大、启动重放或磁盘增长超过预注册预算，架构组必须裁决 ledger segment/checkpoint 策略；不能通过丢历史、跳过 fsync、异步伪装 canonical commit 或恢复旧 snapshot 双轨来“优化”。

## 实施顺序与团队边界

1. **架构组**：冻结本 ADR、canonical field matrix、hash golden vectors、迁移 rejection 语义和 hard erase 边界。
2. **开发组**：实现 ledger/当前视图的唯一持久化路径，迁移 create/update/delete，再删除旧 mutation、compact 与 restore 清空路径。
3. **测试组**：建立 migration/fault-injection/concurrency/restart/hash 门禁，不用穷举掩盖状态机缺陷。
4. **审计组**：枚举所有 `store/delete/clear/persist_memories/compact/restore` 生产调用方，证明没有绕行 writer；检查 trace 隐私和旧格式删除。
5. **研究组**：可并行准备 Memory BM25/vector 消融数据，但不能把候选索引接入 canonical writer。
6. **专家组**：依据硬门禁做 go/no-go；V1-B 通过后才允许 Projection Manifest 进入单路径实现。

## 后果

正面后果：

- 个人记忆纠错拥有稳定身份，历史可验证且不会被检索维护误删；
- projection、thermal 和 Wiki 编译器可按 revision/hash 构建、失效和重建；
- restart、故障恢复和 benchmark 有唯一 canonical watermark；
- 普通遗忘、检索腐败和物理擦除不再混为一种操作。

成本与风险：

- append-only ledger 增加存储、重放与迁移成本，需要安全的 checkpoint/segment 设计；
- 旧数据中损坏或含糊的 supersede 链会阻止自动迁移，需要显式修复而非猜测；
- hard erase 必须遍历 CAS 去重引用与导出副本，不能以普通 tombstone 代替；
- public stable `memory_id`、history、restore、evidence 和 claim 仍需后续独立契约与门禁。

## 拒绝的替代方案

- **继续把 entry ID 同时当逻辑身份和版本身份**：纠错后无法稳定引用同一记忆，也不能可靠失效派生投影。
- **原地覆盖最新 entry**：丢失历史、证据链和可审计性，无法处理时间查询与误更新恢复。
- **用 `superseded_by/deleted_at` 可变字段维持当前视图**：状态有第二真值，crash 与 late task 容易造成不一致。
- **先上 Projection Manifest，再补稳定 revision**：所有 artifact key 和 watermark 都会再次迁移，制造双轨债务。
- **把 compact/TTL 当 hard erase**：把性能策略误作用户删除授权，无法证明擦除完整性。
- **为旧 snapshot 保留兼容 reader/writer**：运行时出现两套真值；本轮目标是清债，不是搬债。
- **把 personal vault 改造成企业多租户 ledger**：超出产品边界，并把同一人的认知角色错误建模为多个数据所有者。
