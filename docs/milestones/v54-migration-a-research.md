# v54 Migration-A 研究证据（Phase A, 外包架构组）

- 日期：2026-08-18；基线 `c64df21`（v53 W0 accepted）
- 方法：隔离 spike `/tmp/rmcp-spike`（不触仓库树），rmcp 自 crates.io 实拉实编，
  对照对象为真实现 `plico-mcp` 二进制（R3.1 线构建，服务端代码此后未变）。
- 全部数字为实测；观测不到的项明确标 OPEN，不虚构。

## 1. 供应链事实（crates.io / GitHub，2026-08-18）

| 项 | 值 |
|---|---|
| 最新稳定版 | **rmcp 3.1.3**（2026-08-17 发布；3.1.0→3.1.3 均在近三周内，release-plz 持续交付） |
| 许可证 | Apache-2.0 |
| MSRV | 1.88（仓库 toolchain 1.95.0 ✓） |
| 协议支持 | 最新规范 2026-07-28，兼容 2025-11-25 及更早；legacy 2024-11-05 HTTP+SSE 为明确非目标（与本仓无关，本仓仅 stdio） |
| 维护状态 | 官方 repo（modelcontextprotocol），活跃（release 自动化、security policy、roadmap） |

依赖树（`cargo tree`，特性 `client,transport-child-process,transport-io`，
`--no-default-features`）：**spike 全图 61 crate，其中相对 Plico（all-features
381 crate）新增仅 8 个**：`rmcp`、`rmcp-macros`、`process-wrap`、`nix`、
`tokio-stream`、`tokio-util`、`futures-executor`、`futures-macro`。
对照组：加 `server` 特性后全图 80 crate、新增 31 个（额外 schemars 全家、
pastey、ref-cast、dyn-clone 等）——这是 GO-partial 不做服务端迁移的直接
供应链理由。

## 2. 互操作实测（rmcp 3.1.3 client × 真 plico-mcp server）

spike 程序：`TokioChildProcess` 拉起 server（stub 后端 + TempDir PLICO_ROOT）。

### 2.1 协商（MCP-A03 部分）

- rmcp 按其默认提议较新协议代；Plico server 恒回 `2024-11-05`；
- **结果：协商成功，`peer_info().protocol_version == "2024-11-05"`**——
  rmcp 接受服务器声明的旧版，无拒绝、无静默错配。serverInfo 读到
  `plico-mcp`，capabilities.tools=true。

### 2.2 exact-14（MCP-A12 部分）

`list_all_tools` 返回 **14 个**工具，名称与 `PUBLIC_OPERATIONS` 完全一致：
capabilities.describe, runtime.readiness, object.put, object.get, object.search,
memory.create, memory.get, memory.recall, projection.status, projection.rebuild,
memory.update, memory.delete, session.start, session.end。

### 2.3 tools/call 不兼容（材料性发现 → MCP-B-R1）

`object.put` 被服务器以 `-32602 "Invalid tool call parameters"` 拒绝。tee 垫片
抓线（wire.log 原始字节）：

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"_meta":{"progressToken":1},"arguments":{"content":"rmcp spike content","tags":["rmcp-spike"]},"name":"object.put"}}
```

rmcp `service.rs` send 路径**无条件**执行
`request.get_meta_mut().set_progress_token(...)`——客户端无开关；Plico 服务端
`ToolCallParams` 为 `deny_unknown_fields`，`_meta` 未知字段即拒。修复归属：
**服务端一行容忍**（ADR-0011 §3.2/MCP-B-R1），非客户端可解。

### 2.4 子进程生命周期（MCP-A07 证据）

spike 进程退出后，server 子进程**仍存活**（pgrep 实证，需手工清理）——
rmcp `TokioChildProcess` 默认 drop 不 kill 不 reap。结论：迁移后客户端必须
**弃用 `TokioChildProcess`**，改用 `transport-io`（流式）+ Plico
`ManagedChild` 唯一持有进程（ADR-0011 §5）。

## 3. 成本数据

- 冷编译（`rm -rf target` 后实测）：client-only 依赖图 **5.9s 墙钟**
  （19.5s user / 5.4s sys，本机多核 arm64；观测值，含并行）。
- 二进制：spike（rmcp+tokio 客户端最小程序）release **4,526,360 B ≈ 4.5MB**；
  对照 plico-mcp release 基线 12,824,880 B ≈ 12.8MB（口径不同：前者仅 SDK 栈，
  后者含全内核）。**plico+rmcp 全链 release 增量 = OPEN**（ADR-0011 §8.3，
  Phase B 前置测量，不以 spike 数外推）。

## 4. 源码级事实（registry 内 rmcp-3.1.3 源）

- 每请求 timeout：`PeerRequestOptions { timeout, reset_timeout_on_progress }`
  存在于 send 路径（`service.rs` ~L880）——deadline 能力内建，Phase B 只需
  封装默认值。
- ID 相关性：`request_id_provider.next_request_id()` + per-request oneshot
  channel（`service.rs` send）——B-01 在 SDK 层解决。
- notification 分流：handler 回调（on_cancelled 等）+ peer 广播，无需手写。
- 自动回 ping：README 声明内建。
- `peer_info()` 返回 `Option<Arc<ServerPeerInfo>>`（协商失败即 None——
  MCP-A03 的 fail-closed 锚点）。

## 5. 开放项（Migration-B 链）

1. rmcp server 侧 × 旧 client 两象限未实测（GO-partial 不依赖）；
2. schemars derive 与手写 exact-14 schema 的 accepted/rejected 集合差异未测；
3. plico+rmcp 全链 release 体积/编译时间未测；
4. `_meta` 容忍后的服务端行为回归（MCP-B-R1 附带 corpus 用例）。

## 6. 结论

证据支持 **GO-partial**：客户端采用 rmcp 3.1.3（8 新 crate，MSRV 合规，
协商/exact-14/ID/deadline 全部达标或有内建能力），服务端保留并加固
（`_meta` 一行容忍 + 既有债务），全量迁移留待 OPEN 项补数后再议。
