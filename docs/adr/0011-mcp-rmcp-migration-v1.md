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

## 4. A2 —— 供应链冻结（GO-partial 口径，A.1 修正版）

- **exact 版本：`rmcp = "=3.1.3"`**（crates.io 2026-08-17 发布；Apache-2.0；
  MSRV 1.88 ≤ 仓库 toolchain 1.95.0 ✓）。**修正后特性集（A1-R01）：
  `client` + `transport-async-rw`，`--no-default-features`**——
  `AsyncRwTransport::new_client(read, write)` 消费 Plico owner 提供的
  tokio 异步流；`transport-child-process`（rmcp 自持 child，与唯一 owner
  冲突）与 `transport-io`（本进程 stdio，server 侧语义）**均不启用**，
  由此 `process-wrap`/`nix` 不进入依赖树。HTTP/SSE/OAuth/auth/base64/
  request-state 一切未用特性继续禁用。
- **最小特性证明（实测）**：剥除 `transport-async-rw` 后 `cargo build`
  失败（E0432 unresolved import `rmcp::transport::async_rw`）；启用错误
  feature 组合会引入 process-wrap/nix（供应链门拒绝）。
- **新依赖面（修正后实测 `cargo tree`）：5 个新 crate** —— `rmcp`、
  `futures-executor`、`futures-macro`、`tokio-stream`、`tokio-util`
  （此前含 child-process 特性的 8-crate 口径作废）。
- **供应链身份绑定（A1-R01.4）**：crates.io tarball
  `rmcp-3.1.3.crate` SHA-256
  `5f17072af977b0f86f714dbd64b3d37d0715bb63064f9d13483f0a1775813374`；
  harness `Cargo.lock` SHA-256
  `b879b7a09b6afa5d1fe5e81d2ce9fbce8575ca89590a7761b14710fc00d59878`；
  使用的 rmcp API 符号：`AsyncRwTransport::new_client`、`ServiceExt::serve`、
  `Peer::peer_info`、`Peer::list_all_tools`、`Peer::call_tool`、
  `CallToolRequestParams`、`ContentBlock::Text`、`PeerRequestOptions`
  （timeout 能力，源码证实）。
- 离线：Phase B 引入方式 = `cargo vendor` 入仓 + `.cargo/config.toml` offline
  source replacement + lockfile 全量校验和；构建期零网络（现有
  `CARGO_NET_OFFLINE=true` 门保持）。
- 成本数据（research §3）：修正特性冷编译墙钟 ~5.9s（61→54 crate 口径）；
  全链 release 增量在 Phase B 候选构建后实测（A.1 冻结测量命令，禁止以
  spike 体积外推整仓）。
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

## 7. 反例 corpus（A1-R04 修正：可执行）

架构拥有的 12 例已从声明式升级为**可执行** harness：
`scripts/milestones/v54/harness/`（独立 crate `reference-adapter` + fixture
服务器 + `run_corpus.py`）。正式验收命令（与 preflight 同一规则实现）：

```
python3 scripts/milestones/v54/harness/run_corpus.py --mode formal
```

最新自测（2026-08-19，本机 offline）：

```
clean:a01..a07b (must-pass): 11/11 pass
mutation:mut-ignore-id / mut-no-deadline / mut-late-response-reuse /
         mut-drop-no-reap / mut-no-wire-cap / mut-loosen-exact14: 6/6 red
summary: executed=17 pass-or-red=17 fail=0 not-run=0
```

冻结常量（twin 与 Phase B 实现共享）：
`MAX_MCP_MESSAGE_BYTES = 1 MiB`（parse 前拒绝，含无分隔符/超长行/分片）、
`INITIALIZE_DEADLINE = 10s`、`REQUEST_DEADLINE = 30s`、
`SHUTDOWN_GRACE = 2s`、`KILL_WAIT_CAP = 5s`、`MAX_INFLIGHT = 64`。
`reset_timeout_on_progress` 冻结为 **false**（持续恶意 progress 不得无限
延长请求）；timeout 仅表示等待终止，不是远端副作用取消。

运行时与生命周期（A1-R02）：单一 runtime 由 adapter 创建持有
（multi-thread，2 workers），同步 caller 经 block_on 桥接，禁止嵌套
block_on 与每请求 runtime；child 由 Plico 侧 ManagedChild（async 端口）
唯一持有，Drop 五状态（初始化中/请求中/panic/EOF/stubborn）均 bounded，
最终 always wait/reap（churn 1000 确定性回收已证）。

**SDK 路径残留（如实登记）**：`transport-async-rw` + 测试持有 child 的
直连 spike 曾在 Phase A 对真实 plico-mcp 二进制全绿（协商/exact-14/错误
路径）；A.1 期间基于 fixture 的 SDK 路径三连测出现挂起（已隔离、未纳入
交付树），根因排查留在 Phase B 首项——不作为接受障碍，但 Phase B 第一个
里程碑必须先让 SDK 路径在 fixture 上全绿再动生产代码。

## 8. OPEN（Migration-B 冓前必须补齐）

1. rmcp **server** 侧最小 spike ×旧 Plico client 的协商/行为实测（两个 OPEN
   象限）；
2. exact-14 手写 schema 与 rmcp/schemars derive 的 accepted/rejected JSON
   集合差异清单（W-01..W-04 一揽子评估的一部分）；
3. release 二进制体积/编译时间完整对照（含 plico+rmcp 全链，而非仅 spike）。

## 9. 非目标

不引入 HTTP/SSE/OAuth/远端发现；不改 Plico 公共语义协议与 exact-14；不以
LOC 为成功指标；SDK telemetry/schema 不入 canonical truth。
