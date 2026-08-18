# v54 MCP SDK Migration-A：官方 RMCP 迁移架构冻结

- 状态：**READY FOR OUTSOURCED ARCHITECTURE / 开发组 BLOCKED**
- 日期：2026-08-18
- 执行方：**外包架构组**
- 接受方：**Plico 架构组 + 安全审计**
- 后续实现方：开发组（仅在 Phase A Accepted 后）
- 基线：v53 W0 accepted tag/commit，由接纳分支 tag 指定

## 1. 目标

评估并冻结从手写 MCP JSON-RPC client/server/schema 迁移到官方 Rust SDK `rmcp` 的单一路径，
修复 response ID 不校验、notification/进度失步、能力协商不足、I/O 无 deadline 与 poisoned mutex
panic 债，同时保持 Plico exact-14、个人本地边界和 managed-child 回收合同。

Phase A **不修改生产 Rust**。它输出 ADR、spike、兼容 corpus、依赖与实现 scope；不得因 SDK
“官方”就预设迁移 GO。

官方输入：

- Rust SDK：https://github.com/modelcontextprotocol/rust-sdk
- MCP specification：https://modelcontextprotocol.io/specification/

当前 Plico 固定 `2024-11-05`；调研时官方 SDK 已进入 3.x/较新协议代际。必须冻结 exact release/
commit 与 protocol negotiation，不能把 main branch 或“latest”写入实现合同。

## 2. Phase A 必交付

### A1. 当前协议与行为清单

逐项记录 client/server 的 initialize、initialized notification、tools/list、tools/call、ping、error code、
ID 类型、notification、stdout framing、stderr、child lifecycle、exact-14 schemas 和 domain error 映射。
对每一项标明：保留、由 rmcp 接管、Plico adapter 保留或明确废弃。

### A2. SDK/供应链冻结

- exact rmcp release/tag/commit、最小 feature set、Rust MSRV、许可证、直接/传递依赖；
- `Cargo.lock` 离线可解析；不引入 HTTP/OAuth/server discovery 等未使用 feature；
- 安全公告与维护状态；升级/回滚策略；
- 比较 release 二进制体积、编译时间与依赖增量，不用“已在 lockfile”替代数据。

### A3. 协议兼容决策

至少对 `2024-11-05` 与拟采用版本给出：

- negotiation/拒绝规则，不静默降级；
- request/response ID 相关性，穿插 notification/progress 时不失步；
- initialize capability 精确值；
- tools schema 的 accepted/rejected JSON 集合与 exact-14 operation 名保持一致；
- parse/invalid request/method/params/internal/domain error 分类不弱化；
- client/server 与旧 Plico/新 rmcp 四向交叉矩阵。

若官方 SDK不能保持必要兼容，Phase A 可返回 **NO-GO / retain hardened adapter**，不得强迁。

### A4. 并发、deadline 与子进程所有权

冻结：

- 每请求 deadline、initialize deadline、shutdown deadline、cancel 语义；
- 同时在途请求的 ID map 与 notification 分流；
- EOF → grace → kill → always wait/reap，覆盖初始化失败、panic、mutex poison、Drop；
- rmcp transport 与 Plico `ManagedChild` 的唯一 owner，禁止双 owner、detached child 或 zombie；
- timeout 后是否仍可能产生外部副作用，不能把 timeout 冒充确定 cancel。

### A5. Exact 实现边界

Phase B 预计只开放：

```
Cargo.toml
Cargo.lock
src/mcp/**
src/bin/plico_mcp/**
tests/mcp_test.rs
tests/mcp_client_test.rs
tests/support/**
```

`api/public`、kernel、scheduler、CAS/memory、exact-14 operation catalog 默认禁止；如 schema derive
确需触碰 public input types，必须单独提出 Architecture Deviation，不能先改后报。

## 3. 架构拥有的反例 corpus

| ID | 场景 | 必须结果 |
|---|---|---|
| MCP-A01 | response ID 错/重复/未知 | typed protocol error，不交给错误 caller |
| MCP-A02 | response 前后穿插 notification/progress | notification 分流，请求仍按 ID 收敛 |
| MCP-A03 | initialize 版本不兼容 | 显式拒绝，无 silent fallback |
| MCP-A04 | tools schema 缺字段/未知字段/边界值 | 与冻结 public input contract 一致 |
| MCP-A05 | request/initialize 永不返回 | deadline 生效，资源有界 |
| MCP-A06 | timeout 后 server 迟到响应 | 不污染下一请求，不误报确定 cancel |
| MCP-A07 | graceful EOF / stubborn child / init failure | bounded 且无 zombie，始终 reap |
| MCP-A08 | poisoned transport state | public call 不 panic；shutdown 仍回收 |
| MCP-A09 | stdout malformed/oversized，stderr 含敏感串 | fail closed；日志不回显正文/secret |
| MCP-A10 | old/new client×server 四向交叉 | 只允许合同声明的兼容组合 |
| MCP-A11 | default-off / MCP 未启用 | kernel/public/CAS 零额外资源或行为 |
| MCP-A12 | exact-14 list/call/domain failure | operation、schema、错误映射与基线等价 |

corpus 必须由架构组拥有，能杀死“忽略 response ID”“无 deadline”“Drop 不 wait”“接受任意版本”
等 mutation；开发组测试只能作为补充 self-evidence。

## 4. 决策与成本门

Phase A 按以下顺序：静态 dependency/API spike → 最小 client/server interop → lifecycle/deadline fault →
schema/exact-14 → 体积/编译成本。前一阶段出现材料性 NO-GO 即停止，不跑重型矩阵。

最终只允许三种裁决：

1. **GO rmcp adapter**：冻结 ADR/spec/corpus/exact Phase-B scope；
2. **GO partial**：只替 client 或 server，另一侧保留并加固，明确单路径与退出条件；
3. **NO-GO retain**：保留手写 adapter，但必须修 response ID、deadline、poison 与测试缺口。

外包架构组只能提交 Proposed ADR/研究证据，不能自称 Accepted。Plico 架构组接受后，另生成开发组
Phase-B 任务单；Phase A 与 Phase B 不得混在一个提交。

## 5. 非目标

- 不引入 MCP HTTP/SSE/OAuth、远端服务或 GitHub 托管门；
- 不改变 Plico public semantic protocol、CAS/memory truth、权限或 Agent 决策；
- 不顺手重构 LLM/embedding/runtime；
- 不把官方 SDK telemetry、schema 或模型内容写入 canonical truth；
- 不以 LOC 减少作为唯一成功指标。
