# v53 R1→WP2 检查点：Durable Store Substrate

**日期**：2026-08-17
**合同版本**：`plico.milestone.v53.wp2/1`
**状态**：Historical B2/A2 freeze / C2 R2 NO-GO / superseded for new work by
[WP2-R2 checkpoint](./v53-wp2-r2-checkpoint.md)
**前置接纳**：R1 candidate `5584b8e7b48247e503d9054bb3b3227c64c7ad94`
**架构接纳记录**：`98de9bd2fa4eb6c6f2dbbb7171ba762124144104`
**实施方**：第三方开发组
**接纳方**：Plico 架构组 + 独立存储/安全/证据专家

本检查点是 [v53 主合同](./v53-execution-observation-spine.md) 的版本化窄化附录，由
[ADR-0008](../adr/0008-execution-observation-store-substrate-v1.md) 授权。旧 R0 v2 packet/A/tag 只证明
WP1 历史，不授权 WP2。

## 1. 唯一目标

实现固定 execution-observation namespace 的 CAS-owned durable transaction substrate，并证明物理写入、
有界读取、双槽 publish 和 F06 crash window 失败关闭。WP2 不实现 attempt/current-view facade、receipt、replay、
recovery policy 或生产接线。

```text
existing Arc<PersonalVaultStorage>
  → architecture-frozen sealed CAS capability
  → private execution_observation::store structural substrate
  → immutable objects + active/candidate pointer slots
```

## 2. 前置证据与不可变边界

新 packet 必须绑定并验证以下提交均为 implementation base 的祖先：

- WP1 accepted candidate：`5584b8e7b48247e503d9054bb3b3227c64c7ad94`；
- dependency-aware scope repair：`2c42b42dac601c9bb6f91ee7db019bf77012a017`；
- architecture-owned WP1 corpus：`9a44c91fec3c870e6a9d8272379da9b748d183bc`；
- R1 acceptance：`98de9bd2fa4eb6c6f2dbbb7171ba762124144104`。

WP1 schema、hash、JCS、validator 和 tests 在 WP2 developer diff 中字节不变。接近 300 行的历史文件不再承载
新功能；架构组在 B2 前完成纯 layout 拆分并以 WP1 corpus 证明行为等价。WP2 新代码从第一天按单概念拆文件。

## 3. Developer exact scope

允许 developer candidate 修改的路径只能由 packet 中 `wp2_spec.json` 的 exact list 给出；不得使用 glob 或
prefix。目标集合为：

```text
src/memory/execution_observation/mod.rs          # 仅激活一个 private store module anchor
src/memory/execution_observation/store/mod.rs
src/memory/execution_observation/store/loader.rs
src/memory/execution_observation/store/publisher.rs
src/memory/execution_observation/store/slots.rs
src/memory/execution_observation/store/tests.rs
```

`src/cas/ledger_store.rs`、`src/cas/mod.rs`、`src/cas/execution_observation_store.rs` 及其 `tests.rs`
由架构组在 B2 冻结，
developer 不得修改。observation `mod.rs` 使用 base→candidate exact byte transformation，不接受同文件内的额外改动。

本检查点对新建 store 文件使用 `<300` 行的窄交付上限，但它不是全仓代码质量定律。拆分必须对应 loader、slot、
publisher 等独立变化原因；禁止为了过线制造 `part1/part2`、增加跨文件跳转或扩大可见性。若单一内聚职责确实
超过上限，developer 必须停止并提交 Architecture Deviation，由架构组决定调整 scope 或记录版本化例外。

## 4. 必须实现

- private sealed structural store；无 public/crate-wide raw writer；
- exact seam 仅为 `FixtureObservationStoreV1::{open_fixture,structural_state,commit_structural}` 和两个
  `#[cfg(test)]` fault injector；commit 接收 typed event/segment/view/root bundle并自行重算 pointer/hash；
- strict fresh/existing namespace open；same-vault single claim；
- `PersonalVaultStorage` 仅可作为 exact `open_fixture` owner 参数并一次性传给 sealed CAS opener，不得访问其
  path、generic ledger 或其他方法；
- pointer/root/segment/stored-event 的 pre-deserialize byte cap；
- stored event cap 135168 bytes；
- canonical JCS、domain hash、sequence/generation/watermark 和 direct reference validation；
- active/candidate closed state；candidate 不得被直接 promote。唯一 `E/P(G0)` 恢复必须从冻结常量重新推导
  exact genesis、验证 candidate bytes 相等后重试 publish，且不得推广到非 genesis；
- publish pre-exchange failure 保持 active bytes；post-exchange uncertainty poison；
- 持久化对象中的 caller/transition/limit 错误统一为稳定 CorruptStore；
- F06 candidate self-evidence，加上 architecture-owned external corpus。

## 5. 禁止实现

- `append_started`、`append_terminal`、`read_attempt`、receipt；
- current-view rebuild、attempt state、duplicate/terminal race、open recovery policy；
- `PersonalVaultStorage::open`、第二把 lock、任意 path/namespace、通用 `ImmutableLedgerStorage`；
- unbounded read/list/collision、chmod/repair existing topology、newest-root selection；
- kernel/scheduler/config/API/bin/MCP/aicli、public export、background worker；
- Memory ledger、projection、KG/vector/trace/event/feedback/cognition；
- 新依赖、Cargo/lock/build/feature 改动、网络或托管 gate。

## 6. R2 对抗验收

1. 累计重跑 WP1 F10/F13、自测与架构 corpus；
2. WP2 F06 和外部 corpus逐个 exact 执行，零测试/ignored/伪输出失败；
3. 默认 lifecycle 与 base mutation inventory 等价，observation namespace/thread/handle absent；
4. non-private/missing/symlink topology 拒绝且 bytes/mode 不被修复；
5. pointer/root/segment/event 的 `limit+1` 在解析前拒绝；
6. existing collision 超限不触发无界 read；
7. pre-exchange fault：active 不变、candidate 只作未采纳 prepared bytes、reopen 不自动 promote；
   fresh genesis 的 `E/P(G0)` 只能通过重算 exact G0 后重试 publish；
8. post-exchange uncertainty：`CommitIndeterminate` 后当前 handle 全部 `Poisoned`；
9. stored malformed/limit/transition 映射为 stable CorruptStore，日志零正文/path/hash；
10. diff scope、crate-private surface、no current-view/replay/live wiring 全部独立复核。

正式 R2 在一次性、低权限、无业务数据、无凭据且禁止网络的本地 sandbox/container 中执行第三方测试；scope
工具的时间、输出和基础 rlimit 只是纵深防御，不等于宿主隔离。无法证明该 runner 边界时，R2 必须保持
PENDING，不得宣称 privacy/host isolation 已验证。

R2 GO 只表示 store substrate 可供 WP3 构建；不表示 ledger facade 或产品能力完成。

## 7. 提交与授权拓扑

```text
R1 acceptance → B2 architecture freeze → A2 approval-only → C2 developer candidate
```

- B2 包含本检查点、ADR-0008、WP2 spec/verifier/corpus和架构 CAS boundary，不含 WP2 approval；
- A2 必须是 B2 的直接单父提交，且只新增 canonical `v53-wp2-approval.json`；
- tag `v53-wp2-v1-<COMMITTED_SHA256>` 必须是直接指向 A2 的 lightweight tag；
- C2 必须以 A2 为祖先；A2 自身因空 implementation diff 不能冒充 candidate；
- packet 留在架构 runner，不交 developer；开发组只拿 Git、A2、tag 与交接页。

任何 schema、scope、CAS capability、error taxonomy 或 external corpus 变化都必须创建新 B3→A3，不得原地
改 approval/tag 或把变化称为续签。

## 8. Packet 到期与续签

Packet 最长有效 14 天；到期后必须 fail closed。若 B2、spec、scope、工具和全部绑定字节未变，架构组从同一
immutable B2 重新 collect，并创建新的 direct-child approval A2′ 与新派生 tag；旧 A2、旧 tag 和旧 packet
保持历史不变。若 C2 已基于旧 A2，开发组只把 implementation diff 重放到 A2′，不得 merge 或修改旧审批。

只要任一冻结输入发生变化，就不是续签：必须形成新 B3→A3→tag 并重新对抗审查。正式授权命令始终显式传入
精确 approval commit SHA，不依赖可移动的默认 remote ref。
