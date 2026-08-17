# v53 WP2-R2 检查点：Durable Store 根因纠偏

**日期**：2026-08-17
**合同版本**：`plico.milestone.v53.wp2-r2/1`
**状态**：Architecture Freeze Candidate / WP2 implementation not started
**审计状态**：C2 NO-GO / R2 blocked
**被审计候选**：`f60eec14da37b107a595f9f93e739a6c06bd6672`（简称 C2 `f60eec1`）
**原冻结基线**：B2 `189f5cf` → A2 `8eb70d7`
**适用合同**：[ADR-0008](../adr/0008-execution-observation-store-substrate-v1.md) 与
[WP2 checkpoint](./v53-wp2-checkpoint.md)

本文件记录 C2 的 R2 对抗审查结论和下一次窄修复必须满足的根因不变量。它不是 approval、packet、tag 或
developer scope；旧 A2 不授权这里新增的 CAS/verifier 变化。架构组必须先形成新的 architecture freeze 与
approval-only 基线，开发组才可提交 C2.1。不得在 C2 上 case-by-case 打补丁后自行宣称 R2 GO。

## 1. 审查裁决

候选 `f60eec1` 的定向自测不能替代冻结 verifier。对 candidate Git bytes 执行独立 scope/surface 审查并推演
并发、stored-reference、genesis、candidate、durability 与 collision race 后，结论为：

```text
C2 f60eec1: NO-GO for R2
WP3: BLOCKED
production/live wiring: still forbidden
```

NO-GO 不否定已经通过的 WP1 wire/hash 或 B2 CAS capability 方向；它表示 structural transaction substrate
尚未证明线性化、完整锚定和所有失败边界，因此不能作为 WP3 的可信 implementation base。

## 2. 七个根因不变量

### R2-R01 — Shared self-preflight first

正式 R2 对任何 developer candidate 的第一步必须复用 architecture-frozen verifier，对 exact candidate Git
object bytes 执行 scope、dependency/import、visibility、crate-private surface 和 exact anchor 预检。预检失败即
停止，不运行会制造“测试绿色”错觉的后续动态 gate。

- developer self-evidence 只能证明候选自测，不授予 scope authority；
- 架构组不得另写临时 grep、简化 scanner 或人工近似规则；
- candidate worktree 内容、生成文件和未提交修复不作为输入；
- 同一 preflight implementation 必须被正式 scope gate 和 external corpus 共同复用。

### R2-R02 — Transaction serialization

同一 handle 内必须有一个 mutex/state machine 覆盖完整线性化区间：

```text
poison check
→ current snapshot
→ bundle validation
→ immutable writes
→ candidate publish / exchange / durability decision
→ in-memory state update
```

只在事务前后分别短锁 state 不构成 single writer。两个 sibling commit 不得基于同一 active head 同时返回
`Ok`，不得由第二次 exchange 把 active 回滚为旧 head。锁 poison 必须 typed fail closed，库代码不得 panic。

### R2-R03 — Validated-before-dereference

来自持久化 bytes 的每个引用必须先完成自身 typed/schema/digest/ordinal 验证，再用于 CAS lookup。特别是 segment
的 `event_sha256`，不得在 segment validation 前解引用，否则非法 digest、错误 schema 或 sequence 会被误报为
I/O/缺对象。

固定顺序为：

```text
bounded read container
→ canonical parse
→ stored semantic validation
→ validate referenced digest form
→ bounded dereference
→ verify referenced bytes/hash
```

实现必须用 private typestate 收敛该顺序，例如 `ParsedRootV1 → ValidatedRootV1`、
`ParsedSegmentV1 → ValidatedSegmentV1`。只有 `Validated*` 暴露可用于下一次 lookup 的 digest/ordinal；loader、slot
classifier 和 publisher 不得直接从裸 serde model 取引用。这样新增字段或调用点也无法绕过验证顺序，而不是依靠
reviewer 逐行记住先后关系。

### R2-R04 — Exact G0 anchor

链尾不能只检查 `(generation, watermark, previous) = (0, 0, None)`。所有 active chain 必须终止于架构重算的
exact G0 root SHA；该 SHA 传递绑定 exact empty current view、`committed_at_ms=0`、空 segment head 与 frozen
schema/trust class。

- `P(G0)/E` 只接受 exact G0；
- hash 自洽但 view、time 或字段不同的 alternate generation-0 root 必须拒绝；
- active chain 终止于 alternate G0 稳定分类为 `broken_root_chain`，不冒充 candidate 关系错误；
- `E/P(G0)` 仍只允许从冻结常量重算并重试正常 publish；
- 不扫描 newest root，不从 orphan 或 candidate 推导替代 genesis。

### R2-R05 — Candidate error boundary

candidate 也是 persisted bytes，不是 caller input。candidate pointer/root/segment/event/view 的 malformed、schema、
transition、digest 或 limit 失败必须在 loader boundary 稳定映射为相应 `CorruptStore` category；物理 read/sync
失败才是 `StorageUnavailable`。

Pointer 分类冻结为：超限=`stored_resource_limit`；非 JCS、缺字段或非法 digest=`noncanonical_pointer`；closed
shape 但 schema 不支持=`unsupported_stored_schema`。只有 pointer 与其对象链各自通过验证后，才进入槽关系分类。

`invalid_candidate_state` 只表示两个分别可解析的槽之间不属于 closed dual-slot relation，例如同 root、非直接
父子或非法 empty/present 组合。不得用它吞掉 object hash、broken chain、noncanonical pointer、stored limit 等
更精确的 corruption，也不得让 stored errors 穿透成 `InvalidRequest`/`TransitionConflict`/`LimitExceeded`。

### R2-R06 — Final flush 裁决

架构裁决：删除 structural publisher 在 `publish_active` 成功后的额外 `storage.flush()`，不新增第三个 fault seam。

原因：每个 immutable object put 已同步 file 与 objects directory；`publish_active` 已同步 candidate bytes、exchange
前 roots directory 和 exchange 后 roots directory。额外 flush 不增加新的 accepted-commit 证明，却制造一个无法
区分且未注入验证的 post-success `CommitIndeterminate` 分支。

保留的故障语义只有：

- exchange 前失败：`StorageUnavailable`，active 不变；
- exchange 后 roots durability 未确认：`CommitIndeterminate`，当前 handle `Poisoned`；
- reopen：重新验证 authoritative active，不返回先前预构造的成功结果。

### R2-R07 — Bounded collision TOCTOU

当前“bounded get → NotFound → generic put”存在竞态：目标可在两步之间出现，generic existing 分支随后进行无界
collision read。修复必须下沉到 CAS 原子 primitive：

```text
validate hash + input cap
→ write private temporary + fsync
→ atomic persist_noclobber
→ success: sync directory
→ Exists: 按 object-kind cap bounded reread/compare
```

Exists 后相同 bytes 才幂等成功；不同 bytes、超限、symlink/special 或变化中的 target 均失败且不修改现有对象。
sealed observation wrapper 绝不能回落到 generic unbounded collision path。

## 3. 修复所有权与 exact 边界

架构组先完成并冻结：

- CAS bounded atomic collision primitive 与 race corpus；
- final flush 删除后的 durability contract；
- exact G0 常量/anchor 与 candidate stored-error oracle；
- 更新后的 verifier preflight、surface policy 和独立并发 corpus。

开发组随后只能在新 approval 基线上修复 observation structural store 的事务序列化、validation order、chain anchor
和 error mapping。不得修改 architecture-owned CAS、spec、verifier、ADR、summary 或本文件。

### 3.1 反复问题的工程化收敛

| 反复失败模式 | 本轮后的唯一机制 | 不再允许的做法 |
|---|---|---|
| developer 自测绿、formal scope 才逐个暴露越界 | formal scope 与 packet-free preflight 共用一个 pure static collector，一次返回全部 stable issue | 复制 grep、只修第一错误、开发组改 verifier |
| persisted bytes 被先解引、后验证 | private `Parsed* → Validated*` typestate；只有 validated object 暴露引用 | 在每个调用点靠代码 review 记忆顺序 |
| sibling commit 分段加锁仍双成功 | 一把 transaction lock 包围唯一 linearization interval | 只锁 in-memory snapshot 或把数据损坏延后给 WP3 |
| bounded pre-read 与 generic put 之间 TOCTOU | CAS 单个 atomic `NOREPLACE`，Exists 后仅 bounded reread | 上层两步存在性检查、调用 generic collision path |
| durability 分支不断增加但无法注入验证 | 最小两窗口模型：pre-exchange 与 post-exchange sync | 重复 flush、为未证分支继续添 injector |
| 旧 approval 被误用来授权新 verifier/CAS | 任何冻结输入变更都产生新 schema/packet/B3→A3/tag | 原地修改 tag、自封 packet、把 developer self-evidence 当 authority |

这些机制是 R2 的交付物，不是对 C2 某些行号的临时豁免。后续候选即使重写内部结构，也必须被同一机制拒绝或接受。

本 checkpoint 继续对新 store 文件使用 `<300` 行的窄交付限额，但它不是 Rust 或全仓质量定律。
出现单一内聚职责无法在该限额内表达时，提交 Architecture Deviation；禁止拆成 `part1/part2`、扩大
visibility 或增加跨文件跳转只为过线。长期质量以单一变化原因、函数复杂度、依赖方向和可测试 seam 为主。

## 4. C2.1 最小验收矩阵

| ID | 对抗输入 | 必须结果 |
|---|---|---|
| R01 | candidate 自测绿但 import/visibility/surface 越界 | shared preflight 首先 NO-GO；动态 gate 不执行 |
| R02 | barrier 同步两个 sibling commit | 至多一个成功；另一方基于新 head 重验后失败；active 不回滚 |
| R03 | segment 携非法/错误-schema event digest | 解引用前 `CorruptStore`；不得变成 NotFound/I/O |
| R04 | hash 自洽 alternate generation-0 root/view/time | active chain 拒绝；只接受重算 exact G0 SHA |
| R05 | malformed/oversized candidate 及合法对象非法槽关系 | pointer/schema/limit 精确分类；仅关系错误为 `invalid_candidate_state` |
| R06 | pre/post exchange fault | 仅两种冻结窗口；无额外 final-flush indeterminate 分支 |
| R07 | NotFound 与 noclobber 之间注入 existing target | bounded reread；零无界 read；target bytes/mode 不变 |

还必须累计通过 WP1 corpus、原 WP2 external corpus、strict topology、default-off lifecycle、privacy/log scan、
fmt/check/clippy 和 exact diff scope。任一 test 为 ignored、零匹配或只打印结果均失败。

## 5. 新冻结拓扑

```text
C2 f60eec1 (R2 NO-GO)
  → B3 architecture remediation freeze（从 A2 重建，不包含 C2 实现）
  → A3 approval-only + `v53-wp2-r2-v1-*` lightweight tag
  → C2.1 developer remediation candidate
  → R2 independent adversarial audit
  → R2 acceptance commit
```

旧 B2/A2/tag/packet 保留历史，不修改、不移动。B3 以 A2 为祖先，只包含架构组的 CAS、verifier、
external corpus 和合同纠偏；不 cherry-pick C2 store 实现。C2.1 必须从新 A3 开始，只重放经新 exact scope 授权的
implementation diff。R2 acceptance 前，[WP3 blueprint](./v53-wp3-blueprint.md) 始终是 Draft/Blocked。

## 6. 停止条件

出现以下任一情况立即停止并回到架构组：扩大 public/crate-wide raw writer、引入任意 path/namespace 或第二把
vault lock、修改 WP1 wire/hash、自动 promote candidate、增加 newest-root/recovery policy、接入 kernel/scheduler、
引入新依赖，或需要用 waiver 跳过上述七项任一根因不变量。
