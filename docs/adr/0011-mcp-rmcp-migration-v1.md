# ADR-0011: MCP SDK Migration —— rmcp 采用决策（Phase A, Proposed）

- 状态：**Proposed（外包架构组提交；仅 Plico 架构组可 Accept）**
- 日期：2026-08-18
- 基线：v53 W0 accepted `c64df2157bd9dcbb1c946704cbda16a08ef4d325`
- 宪章：`docs/milestones/v54-mcp-sdk-migration-a.md`
- 证据：`docs/milestones/v54-migration-a-research.md`（全部数据实测，含 wire 捕获）

## 1. 决策（建议）

**GO-partial：客户端先行采用 rmcp；服务端保留手写实现并加固。**

- 客户端 `McpClient` 迁移到 `rmcp = "3.1.3"`（exact pin，见 §4），
  `transport-child-process` + `transport-io` 特性；
- 服务端 `src/bin/plico_mcp` **本轮不迁**，保留手写 JSON-RPC，但按本 ADR
  §6 完成 `_meta` 容忍（MCP-B-R1）与既有债务修复；
- 全量迁移（服务端 rmcp + schema derive 评估 = Migration-B）保持独立决策点，
  需要本 ADR §8 的 OPEN 项补齐数据后才可提议。

不选择 full-GO 的理由：服务端 rmcp 化会引入 schemars 链（31 个新 crate 中的
大头）与 exact-14 schema 生成语义变化，且服务端象限（rmcp server ↔ 旧
Plico client）无实测数据；GO-partial 的新依赖面仅 8 个 crate（见 §4）。

不选择 NO-GO 的理由：手写 adapter 的四个已知缺陷（response ID 不校验
B-01、无 deadline D-MCP-3、poisoned inherent call panic D-MCP-2、notification
失步）在 rmcp 中均为内建能力（§3、§5 有实测/源码证据），重造代价高于采用。

## 2. A1 —— 现行为清单与归属（client 侧）

| 项 | 现状 | 迁移归属 |
|---|---|---|
| initialize 握手 | 发 `2024-11-05` 常量 | rmcp 接管（发送其首选版本并按 server 回应降级；**实测**接受降级到 2024-11-05） |
| initialized notification | 手动发送 | rmcp 接管（lifecycle 内建） |
| request/response ID 相关性 | **不校验（B-01 债）** | rmcp 接管（oneshot per-id map，源码 `service.rs` send/`id` provider） |
| notification/progress 分流 | 无（锁步读行） | rmcp 接管（A02 由 oneshot map 天然满足） |
| tools/list / tools/call / ping | 手写 | rmcp `list_all_tools` / `call_tool` / 自动回 ping |
| 每请求 deadline | **无（D-MCP-3 债）** | rmcp `PeerRequestOptions::timeout` + `reset_timeout_on_progress`（源码证实存在；Phase B 封装为默认 deadline） |
| 错误分类 | `McpError::{Spawn,Protocol,Io,ServerError}` | Plico adapter 保留（`ServiceError`→`McpError` 映射表，见 corpus MCP-A12） |
| 子进程所有权 | `ManagedChild`（R3.1.1：EOF→grace→kill→always reap） | **Plico 保留**：rmcp `TokioChildProcess` 实测 drop 后子进程残留（research §2.4），故客户端改用 `transport-io`（stdio 流）+ 我方 `ManagedChild` 持有进程，rmcp 只吃流 |
| exact-14 调用面 | `PUBLIC_OPERATIONS` | Plico adapter 保留不变 |
| stderr | `Stdio::null()` | Plico adapter 保留 |

服务端清单：`initialize`（忽略 client 参数、恒回 2024-11-05）、`tools/list`
（exact-14 手写 schema）、`tools/call`（`deny_unknown_fields` 参数校验 +
typed envelope）、`ping`、标准错误码 -32600/-32601/-32602/-32603、行分隔
stdio——**全部保留**，仅按 §6 加 `_meta` 容忍。

## 3. A3 —— 协议兼容（实测数据）

1. **协商**：rmcp 3.1.3 client（默认提议 2026-07-28 代）→ Plico server 回
   2024-11-05 → **协商成功降级**（spike 实测，negotiated=
   `2024-11-05`）。规则冻结：客户端接受 server 回应的任何 `KNOWN_VERSIONS`
   内版本；server 回应未知/畸形版本 → 显式拒绝（corpus MCP-A03）。
2. **tools/call `_meta` 不兼容（材料性发现）**：rmcp 对每个请求**无条件**注入
   `_meta.progressToken`（`service.rs` send 路径硬编码，无开关），被 Plico
   server 的 `deny_unknown_fields` 以 -32602 拒绝（wire 捕获见 research
   §2.3）。**修法（MCP-B-R1，服务端一行）**：`ToolCallParams` 增加
   `_meta: Option<serde_json::Value>`（接受并忽略——服务端不发 progress
   notification，行为诚实）。此为 GO-partial 的硬前置。
3. exact-14：spike 实测 `list_all_tools` 返回 14 个且名称序与
   `PUBLIC_OPERATIONS` 完全一致。
4. initialize capabilities：Plico server `{"tools":{}}`，rmcp client 读取
   正常（`capabilities.tools = true`）。
5. 四向交叉矩阵：old-client×old-server（现状绿）、**rmcp-client×old-server
   （本 Phase 实测：协商/列表/错误路径绿，tools/call 需 MCP-B-R1）**；
   rmcp-server 两个象限 **OPEN**（Migration-B 前置 spike，本 ADR 不预支结论）。

## 4. A2 —— 供应链冻结（GO-partial 口径）

- **exact 版本：`rmcp = "=3.1.3"`**（crates.io 2026-08-17 发布；Apache-2.0；
  MSRV 1.88 ≤ 仓库 toolchain 1.95.0 ✓）。特性：`client`、
  `transport-child-process`、`transport-io`、`--no-default-features`；**禁用**
  HTTP/SSE/OAuth/auth/`base64`/`request-state` 一切未用特性。
- **新依赖面（client-only，实测 `cargo tree`）：8 个新 crate** —— rmcp、
  rmcp-macros、process-wrap、nix、tokio-stream、tokio-util、futures-executor、
  futures-macro（全特性则 31 个：额外 schemars 链/pastey/ref-cast 等，GO-partial
  不引入）。全部为主流维护crate；升级走 minor 安全窗口，major 需新 ADR。
- 离线：Phase B 引入方式 = `cargo vendor` 入仓 + `.cargo/config.toml` offline
  source replacement + lockfile 全量校验和；构建期零网络（现有
  `CARGO_NET_OFFLINE=true` 门保持）。本地 registry 预热不足以满足可复现要求。
- 成本数据（research §3）：client-only 依赖冷编译墙钟 ~5.9s（本机 61 crate
  含共享）；release 二进制增量与基线对照见 research（待后台测量回填，缺数
  不虚构）。
- 回滚策略：adapter 层隔离（`McpClient` 公共 API 不变），保留手写 transport
  一个 commit 距离；回滚 = revert Phase B 单提交。

## 5. A4 —— 并发/deadline/子进程所有权

- **deadline**：每请求默认 deadline（建议 30s）经 `PeerRequestOptions::
  timeout`；initialize/shutdown deadline 由 adapter 层设定；progress 到达可选
  重置。timeout ≠ 确定 cancel（corpus MCP-A06：迟到响应不得污染下一请求）。
- **并发**：rmcp oneshot-per-id + notification 分流内建（A01/A02 由 SDK 满足，
  corpus 仍须证杀 mutation）。
- **子进程唯一 owner**：`ManagedChild`（Plico）为唯一持有者；rmcp 侧用
  `transport-io` 消费 stdin/stdout 流，**不使用 `TokioChildProcess`**（实测
  其 drop 不回收，research §2.4）。EOF→grace→kill→always-reap 合同不变，
  覆盖初始化失败/poison/Drop（沿用 R3.1.1 corpus）。
- rmcp 侧超时后请求句柄行为：timeout 只放弃等待，不保证 server 停止执行——
  adapter 不得把 timeout 冒充 cancel（corpus MCP-A05/A06）。

## 6. Phase B（开发组）预计 scope（供 Plico 架构组接受后另出任务单）

开放：`Cargo.toml`、`Cargo.lock`、vendored 依赖目录、`src/mcp/**`、
`src/bin/plico_mcp/rpc.rs`（仅 MCP-B-R1 `_meta` 容忍一行 + 测试）、
`tests/mcp_test.rs`、`tests/mcp_client_test.rs`、`tests/support/**`。
禁改：`api/public`、kernel、scheduler、CAS/memory、exact-14 catalog、
AGENTS.md；schema derive 触碰 public input types = 先提 Deviation。

## 7. 反例 corpus

架构拥有的 12 例（MCP-A01..A12）已落盘
`scripts/milestones/v54/mcp_migration_corpus.json`，每例绑定必杀 mutation；
开发组测试仅为补充 self-evidence。

## 8. OPEN（Migration-B 冓前必须补齐）

1. rmcp **server** 侧最小 spike ×旧 Plico client 的协商/行为实测（两个 OPEN
   象限）；
2. exact-14 手写 schema 与 rmcp/schemars derive 的 accepted/rejected JSON
   集合差异清单（W-01..W-04 一揽子评估的一部分）；
3. release 二进制体积/编译时间完整对照（含 plico+rmcp 全链，而非仅 spike）。

## 9. 非目标

不引入 HTTP/SSE/OAuth/远端发现；不改 Plico 公共语义协议与 exact-14；不以
LOC 为成功指标；SDK telemetry/schema 不入 canonical truth。
