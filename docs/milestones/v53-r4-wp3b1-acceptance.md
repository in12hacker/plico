# v53 R4：WP3B.1 独立验收里程碑

- 状态：**R4 GO（2026-08-18，Plico 架构组独立验收通过）**
- 日期：2026-08-18
- contract base：`4c957d741e3f78e9c0f2fefec8ed256fb86b25d5`
- developer candidate：`1095ad45fc472f940bdd2e03b46e3cd491954172`
- 执行方：**Plico 架构组 + 安全/存储审计**
- 非执行方：开发组、外包架构组均不得自签 R4 GO

## 1. 当前证据与边界

候选相对 contract base 只改 6 个冻结路径；`wp3b1_verify.py` PASS；12 个 facade 定向用例
在 stub/offline 环境 12/12 PASS。以上是 scope 与 candidate self-evidence，不是独立验收。

本里程碑不接 kernel/scheduler/public/MCP，不宣称真实执行 evidence、身份授权或 VEG；仅裁决
`unverified_fixture` ledger 的 single-writer append、receipt、poison 与 restart 语义。

## 2. R4 前置架构修正（已前向闭合）

冻结的 `wp3b1_spec.json` 含本机绝对 `CARGO_TARGET_DIR`。它不改变 candidate Git diff，
但违反 portable/path-free handoff 规则。R4 架构分支已经：

1. 将当前 spec 前向修订为 runtime `<RUNNER_ROOT>`，不改写旧 tag/历史；
2. 把 packet 的递归检查提升为共用 `reject_nonportable_serialized_value`，WP3B verifier 强制调用，
   拒绝 POSIX/Windows/UNC/Home/checkout/file URI；
3. 用 5 类路径攻击与 portable placeholder 做定向回归。后续 milestone verifier 必须复用该函数。

历史 contract bytes 仍保留旧路径事实，不追溯改写；R4 只认可该前向修订后的 verifier。

## 3. 独立对抗矩阵

架构组从 candidate Git object 建立隔离 checkout，以 architecture-owned fixtures 重放
`wp3b1_corpus` 12 类，并至少加入以下 mutation killers：

| ID | 变异/攻击 | 必须证明 |
|---|---|---|
| R4-M01 | 缩短/移除 facade mutex | barrier sibling commit 被杀死，单 generation 仅一 root |
| R4-M02 | 把 idempotency 判定移到 clock/sequence 后 | retry 的 clock/tree/inventory 零变化断言失败 |
| R4-M03 | receipt 用当前 head/clock 重算 | reopen 与首次 receipt field-exact 不等，变异被杀死 |
| R4-M04 | `CommitIndeterminate` 不 poison | 同 handle 后续 read/write 必须失败，变异被杀死 |
| R4-M05 | restart/read 混入 candidate | staged valid child 不进入 read/reducer，不被 promote |
| R4-M06 | terminal rebind 在 policy/runtime 检查前走幂等 | outcome/evidence/policy/runtime 任一变化均 typed conflict |
| R4-M07 | reducer 出现第二实现或绕过 stored validation | startup/append/read 三路径对同 corrupt chain 分类一致 |
| R4-M08 | default construction eagerly open namespace | 未调用 facade 时 vault/public exact-14/MCP surfaces 零差异 |

同时真实运行 pre-exchange、post-exchange、reopen、clock rollback/overflow、五种 terminal outcome、
same/different request 并发；不能只检查测试名或复用开发组断言。

## 4. 低成本 gate 顺序

1. Git ancestry、exact diff、`git diff --check`、path-free validator；
2. 冻结 scope verifier；
3. architecture-owned 12 类 corpus + 8 个 mutation killers；
4. targeted fmt/check/clippy；
5. 只有 1–4 全绿才运行全库 lib/offline 回归与 coverage。

该顺序用于控制费用；任何真值/持久化 P1 出现立即停止重型 gate。

## 5. 裁决规则

- P0 或真值/持久化/权限 P1：R4 NO-GO，退回窄 remediation；
- 局部、独立、已有 owner/test/deadline 的 P2：登记后滚入下一个大开发，不阻断；
- GO 必须记录 candidate SHA、architecture fixture digest、原始计数、环境边界和已知限制；
- 开发组自测、调研报告或“架构接受”文案均不能替代上述独立证据。

## 6. 已登记的可滚动债

| Debt | 严重度 | Owner | 截止 |
|---|---|---|---|
| append 每次 clone/reduce 全事件，最坏 O(n²) | P2 性能 | 外包架构组先给 snapshot/index 方案，开发组后实现 | WP3B.2 前 |
| facade 593 行、tests 904 行 | P2 可维护性 | 开发组按职责自然拆分 | R4 后最近一次相关大开发 |
| D-MCP-2 poisoned inherent `call_tool` panic | P2 独立 transport 债 | 外包架构组 | MCP migration-A |
| D-MCP-3 MCP I/O 无 deadline | P2 独立 transport 债 | 外包架构组 | MCP migration-A |

300 行仅为 review trigger；不得为了数字扩大 visibility 或切成无语义的 part 文件。

## 7. R4 后的自然下一程

R4 GO 后先交 **开发组** 执行 W0 rolling hygiene（UTF-8 截断、provider usage、死代码/文档、
小型仓内去重）。MCP/rmcp 迁移先交 **外包架构组** 冻结 ADR/corpus，之后才交开发组实现。
持久化后端替换只进入 research branch，不与 v53 ledger 主线并行改真值。

## 8. R4 执行记录（2026-08-18，独立验收，GO）

执行环境：隔离 checkout `/tmp/plico-v53-r4-probe`（候选 Git object 1095ad4），
专用外置 target dir，offline + stub 后端；架构探针源文件 SHA-256
`824f3ce5fb08a845cf394f830d233a6376f7c45f63ae5db033120c033dc207c9`
（保留于该工作区，与 R3 证据工作区同纪律，不入候选树）。

### Gate 1–2（静态）

- 拓扑：contract 4c957d7 → candidate 1095ad4 → audit 46309c1 祖先链成立；
  候选 diff 恰为 6 个冻结路径（+1697/−153），`git diff --check` 干净。
- path-free validator 5 类攻击（POSIX/Windows/UNC/home/file URI）全部拒绝，
  portable 占位符放行；scope verifier 对候选 PASS。

### Gate 3（独立 corpus + mutation）

架构自有探针（自有 UUID/hex fixture 族，不复用开发组断言）14 例
（C01–C13 对应语料 12 类 + 突变期新增 C14 同请求 started 竞速）在干净候选上
**14/14 连续 4 轮全绿**。

| Mutant | 结果 | 杀伤探针 |
|---|---|---|
| M01 缩短 facade mutex（校验与提交间解锁重锁） | KILLED（3/3 确定性） | C14 |
| M02 移除 started 幂等早退 | KILLED | C01/C05/C10 |
| M03 receipt 改用当前系统时钟 | KILLED | C01/C03/C05/C06/C07/C10/C11 |
| M04 CommitIndeterminate 不 poison | KILLED | C08 |
| M06 bound-terminal 幂等先于 rebind 校验 | KILLED | C04/C11 |
| M07 facade 层 corrupt→availability 分类漂移 | SURVIVED（定因，见下） | — |

M07 存活定因：改坏 facade 侧分类后 C13 仍得到 `CorruptStore` ——可观测分类由
架构冻结的 store 开期 typestate 校验先行负责（不在候选 6 文件内，超出候选突变
范围），facade 侧是对称的冗余校验；组合系统对篡改对象字节 fail-closed 由 C13
在干净代码证明。M05（candidate 混入 read/reducer）：候选范围内结构性不可突变
——readonly 能力（R3 冻结）不暴露 candidate API，facade 不触 storage 内部件；
运行时由 C09 双分支证明（垃圾 candidate 要么被冻结分类拒绝要么被忽略，字节
零改动）。M08（default-off eager 资源）：C12 运行时（namespace 不因模块包含而
创建）+ 静态 grep 六文件零 eager initializer + exact-14 集成测试逐字不变。

### Gate 5（收尾回归）

- 候选树全库 `--lib`：**2128/0/2**；`--lib --all-features`：**2146/0/2**
  （与开发组申报计数一致，独立复现）。
- exact-14：`client_discovers_exact_public_tool_catalog` PASS；
  mcp integration 10+3 PASS。

### 探针缺陷日志（诚实记录）

审计期间探针自身缺陷 6 处（非法 UUID 族、非 hex 盐参数、TempDir 提前释放、
c11/c14 悬空 Arc clone 致 flock 持有、c07 未计合法 pre-exchange 孤儿对象、
c09 垃圾 candidate 语义双分支化），逐一修复后才获得上述全绿；候选实现在本
审计中未发现任何需要修复的缺陷。

### 裁决

- P0：0；真值/持久化/权限 P1：0。
- 新增登记（P2，滚入既有债务表）：无新增；O(n²) append 与文件长度已在 §6。
- 已知限制：D1 家族负载敏感 flake 为环境性（本轮未复现于 Gate 5 窗口）；
  facade 侧分类冗余不可达（见 M07 定因）。
- **R4 GO**：candidate `1095ad45fc472f940bdd2e03b46e3cd491954172` 通过独立
  corpus、mutation 与回归验收。WP3B.1-B 交付被接受；开发组可接 W0 滚动清债。
