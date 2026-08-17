# v53 验收摘要：Execution Observation Ledger Core

**状态**：R1 Model/Hash accepted / WP2 architecture freeze in progress / R2 not started
**合同**：[v53-execution-observation-spine.md](./v53-execution-observation-spine.md)
**WP1 历史交接**：[v53-developer-handoff.md](./v53-developer-handoff.md)
**WP2 当前交接**：[v53-wp2-developer-handoff.md](./v53-wp2-developer-handoff.md)
**产品基线**：`fe4c08260fc3e6dc0e3d37921b863a7ed48a330a`

> 本文件只由 Plico 架构/验收组填写；R0 冻结项可在开发开工前记录，其余项只在候选提交与独立证据完整后填写。
> 第三方开发组不得修改本文件或预填通过状态、测试数字、benchmark 结论；只能提交独立的候选证据输入。

## 1. 状态

- Architecture-Frozen：✅（R0 v2 packet + approval `eb23261084b2b7a38a40540ecfffd3cb327c54fa`）
- Implementation-Candidate：✅（R1 candidate `5584b8e7b48247e503d9054bb3b3227c64c7ad94`）
- WP2-Architecture-Frozen：⬜（由 WP2 packet + approval-only A2/tag 动态确定）
- Evidence-Complete：⬜
- Final decision：`PENDING`

## 2. 冻结身份

| 项目 | 值 |
|---|---|
| Accepted narrow ADR | `docs/adr/0007-execution-observation-ledger-v1.md`；digest 由外部 R0 packet 绑定 |
| Contract digest | 由外部 R0 packet 绑定 |
| Architecture-Frozen implementation base | `a86ad4c450762138eea13bc1a39f045a45b67b24` / tree `0485d6819fe2d9d34a7591cda984186a6354e950` |
| Candidate scope base | `eb23261084b2b7a38a40540ecfffd3cb327c54fa`；离线授权器 `GO` |
| Candidate commit | `5584b8e7b48247e503d9054bb3b3227c64c7ad94` / tree `f5e798e4b0cff2a9511e29c7a791477f873b8c30` |
| Worktree clean | 独立 Git-object materialization + clean checkout 验证；共享工作树 `.zcode/` 不计入 candidate |
| Cargo.lock digest | `a6f237ada517e77b3b006e9b5a3ba1e5645c5b8ba992ccc9e9b42e1d07125792`（100,222 bytes） |
| benchmarks/uv.lock digest | `a2d42a133228ef6e12a5362c2d2f04385e9abf3ef08a8d65da9a7664a62722ec`（423,717 bytes） |
| R0 handoff packet digest | `5f5ff083dc07a744c48d5df8e6be3d6937cec4ffebe6d458b24299ecd81bb1de`（COMMITTED） |
| R1 independent corpus | verifier `9a44c91fec3c870e6a9d8272379da9b748d183bc`；source SHA-256 `7a7dde3e55044fb8566cffb659e3c3d8c8c68950a50172075dac802031629eca`；WP6 sealed bundle 仍 PENDING |
| R0 packet collector/verifier version | `plico.v53.r0-spec/v2` / `plico.v53.r0-handoff/v2`；v1 已撤销、不得复用 |
| WP2 checkpoint contract | `plico.milestone.v53.wp2/1`；ADR-0008；旧 R0 packet/A1 不授权 WP2 |
| R0 packet integrity / Git authorization | verified / GO（程序性、unsigned；限制仍按合同）；WP2 packet/authorization PENDING |

## 3. 不变量验收

| 项目 | 结果 | 独立证据 |
|---|---|---|
| production zero wiring / no observation-added mutation | PENDING | PENDING |
| canonical-form CID refs / fixed unverified fixture / no false authorization claim | PENDING | PENDING |
| Started/Terminal state machine | R1 PASS | candidate tests + independent counterexample replay |
| idempotent retry / terminal conflict | R1 PASS | body-derived request hash、三方 key、policy/runtime binding |
| Open/Terminal restart equality | PENDING | PENDING |
| writer poison / post-publish uncertainty | PENDING | PENDING |
| concurrent single terminal | PENDING | PENDING |
| JCS/domain hash/future schema | R1 PASS | 7 domains、golden chain/pointers、JCS-first combined attacks |
| privacy/log scan | PENDING | PENDING |
| exact-14/catalog golden/deterministic response semantics unchanged | PENDING | PENDING |
| Memory/projection/KG/skill/retrieval unchanged | PENDING | PENDING |

## 4. 门禁

| 门禁 | 结果 |
|---|---|
| cargo fmt | R1 PASS |
| cargo check all targets/features | R1 PASS |
| cargo test lib/all features | R1 targeted PASS；全库 sandbox 基线有既有 loopback/deadline failures，不归因 candidate |
| cargo test all | PENDING |
| cargo clippy `-D warnings` | R1 PASS |
| coverage：local floor ≥85%；candidate total ≥85.83%；observation module ≥95% | PENDING |
| perf regression | PENDING |
| benchmark ruff/pytest | PENDING |
| diff/forbidden-path audit | R1 PASS：9 exact paths；scope base→candidate clean materialization |

## 5. Review

| Checkpoint | Reviewer | Verdict | Notes |
|---|---|---|---|
| R0 Constitution/ADR | Plico architecture + independent QA/red-team | GO | packet v2 + approval/tag 离线验证 |
| R1 Model/Hash | Plico architecture + data-integrity/red-team review | GO | candidate `5584b8e`; 33/33 targeted; repaired scope PASS |
| R2 Store Substrate | PENDING | PENDING | fixed namespace、bounded CAS capability、structural commit、F06；不含 facade/receipt |
| R3 View/Identity boundary | PENDING | PENDING | |
| R4 Fault/Admitted boundary | PENDING | PENDING | |
| R5 Adversarial QA | PENDING | PENDING | |
| R6 Final alignment | PENDING | PENDING | |

## 6. 最终结论与限制

PENDING。即使最终 GO，也只能声明 internal, unconnected observation ledger core；不得声明 trusted
producer/evidence authorization、真实执行 coverage、public capability、自动学习、branch runtime 或
Verified Experience product gate。

## 7. Deviations / Waivers / Named debt

| 类型 | ID | Owner | Due date | 状态 | 说明/证据 |
|---|---|---|---|---|---|
| Architecture Deviation | V53-R1-D01 | Architecture | before WP2 approval | CLOSED | R0 scanner把 nested use tree/合法相对路径当 root；`2c42b42` 改为 use-tree 展开、词法 module depth 与正向 capability policy；`9a44c91` 加入独立只读 overlay corpus；同 candidate scope PASS |
| Local gate waiver | V53-R1-W01 | Architecture | R1 only | CLOSED | 全库 sandbox 的 loopback bind/deadline 失败由 candidate 前基线复现；R1 使用 clean targeted/check/clippy + 独立 diff，不把环境失败涂绿 |
| Named debt | V53-WP2-D01 | WP2 | R2 | OPEN / hard gate | stored-read caller/transition/limit 错误在 loader 边界统一映射稳定 `CorruptStore`；resource cap 为 `stored_resource_limit` |
| Named debt | V53-WP2-D02 | WP2 | R2 | OPEN / hard gate | raw bytes 在反序列化前按 object kind bounded read；stored event cap 135168；廉价 count gate 先于分配/写入 |
| Named debt | V53-WP2-D03 | WP2 + WP3 | R2/R3 | SPLIT | R2 验证 structural sequence/generation/watermark 与直接引用；全量 event→attempt view replay 由 WP3 完成 |
| Named debt | V53-WP2-D04 | WP3 | R3 | DEFERRED BY ADR-0008 | receipt 只在 WP3 从全量已验证链派生；WP2 明确不返回 receipt |
| Architecture maintenance | V53-WP2-D05 | Architecture | before WP2 approval | CLOSED | WP1 model/tests/helper 已按 request/event/ledger 与测试职责做纯 layout 拆分；WP1 33/33 unchanged + architecture CAS 6/6（combined filter 39/39），check/clippy/fmt 通过；developer WP2 不得修改这些 bytes |

R1 reviewer record：Plico architecture group + independent contract/evidence/red-team reviewers，2026-08-17。
架构修复 verifier commits 为 `2c42b42dac601c9bb6f91ee7db019bf77012a017`、
`9a44c91fec3c870e6a9d8272379da9b748d183bc`；它们必须被下一份 checkpoint
packet/spec 绑定后才能授权 WP2，旧 R0 packet 不得被解释为已授权后续工作包。
