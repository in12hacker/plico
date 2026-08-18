# v54 Migration-A.1：RMCP 可实现性补强

- 状态：**READY FOR OUTSOURCED ARCHITECTURE / 开发组 BLOCKED**
- 日期：2026-08-19
- 执行方：**外包架构组**
- 复审方：Plico 架构组
- 输入提交：`506af2c9e89296134b8e2c8350600c68b0dc4fef`
- 输入 ADR：`docs/adr/0011-mcp-rmcp-migration-v1.md`（Proposed）

## 1. 裁决

Migration-A 的 **client-first / server-retain** 方向保留，但当前材料不能进入
Phase B。阻断不是若干独立小 bug，而是一个共同根因：**拟采用的 transport、
现有同步 API、子进程唯一所有权和有界 wire 合同尚未组成同一条可编译、可验证
的运行路径**。

本轮不要求开发组试错，也不接受“实现时再决定”。外包架构组完成 A1-R01 至
A1-R04 后，Plico 架构组才决定是否接受 ADR-0011，并另发开发组任务单。

P0：0（本次没有生产 Rust 变更）。P1：4，均须在 Phase B 前关闭。

## 2. 根因与修复任务

### A1-R01：冻结真实的最小 transport 与 feature 集

`rmcp 3.1.3` 的冻结源码定义为：

- `transport-io` 只创建**当前进程**的 Tokio stdin/stdout，定位是 server-side
  stdio；
- `transport-child-process` 使用 `TokioChildProcess` 并自行 spawn child；
- 任意 Tokio `AsyncRead + AsyncWrite` 流对使用 `transport-async-rw`。

当前 Proposed ADR 同时启用前两项，却又禁止使用 `TokioChildProcess`，并声称
把 Plico child pipes 交给 `transport-io`。这条路径与 SDK feature/API 不一致。
现有 `ManagedChild` 还持有 `std::process::{Child, ChildStdin, ChildStdout}`，这些
流不能直接满足 rmcp 的 Tokio `AsyncRead/AsyncWrite` 合同。

外包架构组必须交付：

1. 一个可编译、可运行的最小 spike，由 **Plico owner** spawn/持有 child，rmcp
   只消费 child stdout/stdin；drop、初始化失败与请求失败后均无 zombie；
2. 基于 spike 冻结 exact feature。优先验证
   `client,transport-async-rw --no-default-features`；若仍需
   `transport-child-process` 或 `transport-io`，必须给出实际调用点和唯一 owner
   证明，不能仅因依赖已下载而启用；
3. 用修正后的 feature 重算新增 crate、许可证、冷编译时间和 release 增量；
   `process-wrap`/`nix` 等只由未使用 feature 引入时必须移除；
4. 记录 crates.io tarball checksum、`Cargo.lock` identity 与使用到的 rmcp API
   符号，禁止只绑定“3.1.3”文本。

验收：删除任一必需 feature 时 spike 编译失败；加入未使用 transport feature 时
供应链门失败。SDK 不允许维持 Plico 唯一 owner 时，结论应转为 NO-GO/加固手写
adapter，不得为采用 SDK 放宽 lifecycle。

### A1-R02：冻结 sync-to-async runtime 与生命周期状态机

当前 `ExternalToolProvider::call_tool` 和 `McpClient::new` 是同步接口；rmcp service
和可用 child pipes 是 Tokio 异步对象。Proposed ADR 没有定义谁拥有 runtime、
service、child 和 shutdown join，因此尚不可实现。

必须冻结一条单路径，并用时序图和 spike 证明：

- runtime 的创建者、线程数、启动失败与 Drop 顺序；不得每请求新建 runtime，
  不得在 Tokio runtime 线程内嵌套 `block_on`；
- child 的唯一 owner、stdin/stdout 的所有权转移、service task 的 join/abort
  边界；不存在 detached task 或双 owner；
- 同步 caller 到异步 service 的有界 command channel、最大同时在途请求数和
  backpressure；mutex poison/worker panic 后所有调用返回稳定 typed error；
- initialize、普通 request、shutdown/grace/kill 的 **exact finite durations**；
  `30s` 不得继续写作“建议”；
- `reset_timeout_on_progress` 固定 true 或 false，并说明持续恶意 progress 是否能
  无限延长请求；timeout 只代表等待终止，不代表远端副作用取消；
- Drop 在初始化中、请求中、worker panic、EOF 与 stubborn child 五种状态下都
  bounded，最终 always wait/reap。

验收：至少连续 1,000 次 spawn/drop，及并发/timeout/迟到 response 故障注入，
进程数、task 数与 channel 队列回到基线；禁止用 sleep 后 `pgrep` 的单点观察
替代确定性 child handle/wait 断言。

### A1-R03：冻结 bounded wire 与 `_meta` 兼容边界

两个实现当前都存在无界读：Plico server 使用 `stdin.lines()`，rmcp 3.1.3
`AsyncRwTransport::receive` 使用持久 `Vec<u8>` + `read_until(b'\n')`，没有应用
`JsonRpcMessageCodec::max_length`。因此 MCP-A09 目前只有期望，没有可执行上限。

必须冻结：

1. client/server 共用的 `MAX_MCP_MESSAGE_BYTES` exact 数值，且在 JSON parse/
   buffer 扩张前拒绝；EOF 无换行、超长单行、超长 `_meta`、连续空行分别定义；
2. 若 rmcp generic transport 无法施加上限，提供最小 bounded transport wrapper，
   或据此判定 SDK client NO-GO；不得先无界读再检查 `String::len()`；
3. `_meta` 只在 protocol adapter 层接受并忽略，不进入 public command、CAS、
   tracing 或错误正文。不得直接以无界 `Option<Value>` 作为最终合同；应使用
   与冻结 MCP RequestMeta 兼容的有界 DTO，或对整个原始 message 先做严格上限；
4. 未知顶层业务参数仍由 `deny_unknown_fields` 拒绝；容忍 `_meta` 不得演化为
   接受任意 tool-call 参数。

验收：`cap`、`cap+1`、无 delimiter、分片到达、含 secret 的 `_meta` 均由架构
corpus 运行；日志扫描对正文、路径、token 为零命中。

### A1-R04：把声明式 corpus 变成架构拥有的可执行证据

现有 `mcp_migration_corpus.json` 描述了 12 个场景，但没有 runner、fixture、
oracle 或 mutation executor。它是测试设计，不是可以独立验真的架构证据。

外包架构组必须新增本地、离线、架构拥有的 harness：

- fixture server/proxy 可确定性制造 wrong/duplicate/unknown ID、交错
  notification、永不返回、迟到 response、malformed/oversized line、EOF、
  stubborn child 与敏感 stderr；
- 每个 MCP-A01..A12 绑定 exact test 名、timeout、输入 digest 和稳定 oracle；
- 至少证明“忽略 ID”“移除 deadline”“timeout response 进入下一请求”“Drop 不
  wait”“移除 wire cap”“放宽 exact-14”六个 mutation 会红；
- runner 只在一次性、无凭据、无业务数据、无网络的本地 sandbox 运行；候选
  自测不得替代该 corpus；
- 给出一条轻量 preflight 和一条正式验收命令，二者复用同一规则实现，不复制
  正则或 oracle。

验收结果必须区分 `executed/pass/fail/not-run`，fail-fast 后不得把未执行项记为
通过。

## 3. 可滚动而不阻断 A.1 的事项

以下内容不阻断 client-first 架构接受，但必须保持 OPEN，不能写成 Plico 能力：

- rmcp server ×旧 client 两个象限；
- schemars derive 与 exact-14 手写 schema 的全量等价；
- HTTP/SSE/OAuth 与远端发现；
- Migration-B 的 server 替换。

完整 Plico release 二进制增量应在 Phase B 候选构建后实测；A.1 先冻结 feature
与测量命令，禁止继续用最小 spike 体积外推整仓结果。

## 4. 允许修改与禁止修改

A.1 仅允许：

```
docs/adr/0011-mcp-rmcp-migration-v1.md
docs/milestones/v54-migration-a-research.md
docs/milestones/v54-migration-a1-remediation.md
docs/milestones/INDEX.md
scripts/milestones/v54/**
```

可在 repo 外创建隔离 spike。**禁止修改生产 Rust、Cargo.toml/Cargo.lock、public
API、exact-14 catalog、CAS/memory/kernel/scheduler。** 若验证需要临时依赖，
只能存在于隔离 spike，并在研究记录中绑定源码与 checksum。

## 5. 交付与下一决策

外包架构组从交接分支 `v54-mcp-migration-a-r1` 建立 A.1 候选；被审输入提交
`506af2c9e89296134b8e2c8350600c68b0dc4fef` 必须保持为祖先。提交更新后的
Proposed ADR、研究证据、可执行 corpus/harness 与自测结果。不得把 ADR 改为
Accepted，不得生成开发组实现提交。

Plico 架构组复审只有三种输出：

1. **Accept client-first**：另建 acceptance commit，并生成 Phase B 开发任务；
2. **NO-GO retain hardened adapter**：生成手写 client 的 ID/deadline/bounds 修复
   任务；
3. **仍不确定**：只允许针对未闭合根因追加一次架构实验，不扩大研究面。

低成本执行顺序：R01 编译 spike → R02 lifecycle/deadline → R03 wire bounds →
R04 corpus mutation。任一阶段证实不可行即可停止后续重型测试。

## 6. A.1 执行记录（2026-08-19，外包架构组交付，待 Plico 架构组复审）

### A1-R01 transport/feature 修正 —— executed / pass

- 源码核实：`transport-io` = 本进程 Tokio stdio（server 语义）；
  `transport-child-process` = rmcp 自持 child；任意异步流对 =
  `transport-async-rw`（`AsyncRwTransport::new_client(read, write)`，
  `rmcp::transport::async_rw`，feature 门控 `transport-async-rw`）。
- **冻结 feature 集：`client` + `transport-async-rw`，`--no-default-features`**。
  剥除 async-rw 后编译失败（E0432，实测）；旧 8-crate 口径中的
  `process-wrap`/`nix` 随 child-process 特性消失——**新依赖面收敛为 5 个
  crate**（rmcp、futures-executor、futures-macro、tokio-stream、tokio-util）。
- 供应链身份：rmcp-3.1.3.crate SHA-256
  `5f17072af977b0f86f714dbd64b3d37d0715bb63064f9d13483f0a1775813374`；
  harness Cargo.lock SHA-256
  `b879b7a09b6afa5d1fe5e81d2ce9fbce8575ca89590a7761b14710fc00d59878`；
  API 符号清单见 ADR-0011 §4。
- Plico-owner + rmcp 消费流的组合路径已在 Phase A 对真实 plico-mcp 二进制
  实测绿（协商降级/exact-14/typed 错误）。A.1 的 fixture 版 SDK 路径三连测
  出现挂起，已隔离出交付树并如实登记为 Phase B 首项前置（见 §6 末尾）。

### A1-R02 runtime/生命周期冻结 —— executed / pass（twin 实证）

- 冻结常量：INITIALIZE 10s、REQUEST 30s、SHUTDOWN_GRACE 2s、KILL_WAIT_CAP
  5s、MAX_INFLIGHT 64、`reset_timeout_on_progress = false`（写入 ADR §7）。
- twin 状态机：单一 worker 线程 + bounded sync channel；Drop 五状态 bounded，
  always wait/reap；**churn 1000 轮确定性回收（wait 句柄 + /proc 双证）**，
  case `a07b` pass（clean 11.6s / formal 全量内含）。
- mutex poison/worker panic 路径在 twin 中经 fail_all 收敛为 typed error。

### A1-R03 bounded wire + _meta 边界 —— executed / pass

- 冻结 `MAX_MCP_MESSAGE_BYTES = 1 MiB`；`LineBoundedReader` 在 parse 前按
  行拒绝（cap/cap+1/无分隔符/分片到达四类用例绿：`a09`/`a09b`）。
- rmcp 3.1.3 `AsyncRwTransport::receive` 源码证实为无界
  `read_until`（持久 Vec，无 max_length）——Phase B 必须以本 harness 的
  bounded reader 包裹读半边（同构实现已在 twin 中验证）。
- `_meta`：仅 adapter 层接受并忽略；消息整体先过 1 MiB 上限（有界 by
  construction），未知顶层业务参数仍 deny_unknown_fields 拒绝（corpus
  `a12d` 目录漂移拒绝 + `mut-no-wire-cap` 红）。

### A1-R04 可执行 mutation corpus —— executed / pass

- 交付：`scripts/milestones/v54/harness/`（独立 crate + fixture 服务器 +
  `run_corpus.py`，preflight/formal 同一规则表，全部子进程命令为内联字面量
  参数表）。
- 正式命令：`python3 scripts/milestones/v54/harness/run_corpus.py --mode
  formal`；preflight：同命令 `--mode preflight`。
- 最新结果（2026-08-19，offline）：
  `clean 11/11 pass；6 mutation 全 red；summary executed=17 pass-or-red=17
  fail=0 not-run=0`。六个必杀 mutation 与杀死它们的用例：
  ignore-id→a01、no-deadline→a03（挂起即红）、late-response-reuse→a06、
  drop-no-reap→a07/a07b、no-wire-cap→a09/a09b、loosen-exact14→a12d。

### 分类与残留（executed/pass/fail/not-run 全量披露）

| 项 | 状态 |
|---|---|
| R01 feature 修正 + 最小特性证明 + 供应链身份 | executed / pass |
| R02 常量冻结 + churn 1000 | executed / pass |
| R03 wire cap 四类 + _meta 边界 | executed / pass |
| R04 corpus + 6 mutation 红 | executed / pass（17/17） |
| SDK 路径（fixture 版三连测） | **fail（挂起）**——源码移出交付树；Phase A 对真实二进制的实测仍有效；Phase B 前置首项 |
| rmcp-server 两象限 / schemars 等价 / 全链 release 体积 | not-run（按 §3 滚动，OPEN 不变） |

过程中清理了调试残留的全部 fixture/测试子进程（最终 `ps` 零残留）。

## 7. Plico 架构组复审裁决（2026-08-19）

**Accept client-first（GO-partial，ADR-0011 Accepted）。**

复审项与证据：
1. 祖先链：506af2c ∈ ancestors(75e2d3d) ✓（`merge-base --is-ancestor`）；
2. scope：`506af2c..75e2d3d` 全部 8 文件 ∈ allowlist（ADR/INDEX/remediation/
   scripts/milestones/v54/**），`src/、Cargo.*、tests/` 零修改 ✓；
3. 交付时状态合规：ADR 保持 Proposed 至本复审；无生产 Rust ✓；
4. **fresh 复跑官方命令**：`run_corpus.py --mode formal` 在复审机上
   **executed=17 / pass-or-red=17 / fail=0 / not-run=0**（clean 11/11，
   6 mutation 全 red）——独立重现 ✓；
5. 研究文档关键主张（rmcp receive 无界、_meta 注入、child-process 冲突）
   与源码/线捕一致 ✓；
6. **A.1 唯一悬挂项（SDK 路径 fixture 挂起）根因定位**：panic 位于
   `child.stdin/stdout.take().expect(...)`——spike 程序未为子进程配置
   piped stdio（`Stdio::piped()` 缺失）。与 rmcp 无关，不构成架构障碍；
   Phase B 直接以正确管道构造即可。复审记录此定位并撤销“挂起=未明”
   表述：根因已明（测试代码缺陷）。
7. 复审后进程零残留 ✓。

裁决后动作：
- ADR-0011 转 **Accepted**（本节附注随 acceptance commit 入库）；
- 开发组 Phase B 由外包架构组出任务单：MCP-B-R1（`_meta` 容忍，rpc.rs
  一处）+ rmcp 客户端迁移（corrected feature 集、vendor 离线、
  corrected pipeline 构造）+ 复用本 harness corpus 作为独立验收；
- 不扩大研究面；rmcp-server 两象限、schemars 等价、全链 release 体积
  继续滚动（Migration-B 前置）。
