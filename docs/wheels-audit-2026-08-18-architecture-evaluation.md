# 造轮子审计 — 架构评估（2026-08-18）

状态：**Architecture Decision Input / 非实现授权**  
对象：`wheels-audit-2026-08-18.md` 与开发组交叉核验记录  
原则：替换基础设施轮子，但不把 Plico 的真值、防篡改、授权和可重放语义外包给通用库。

## 1. 裁决摘要

调研的库存和缺陷线索总体可信，但“成熟 crate 存在”不等于“立即替换”。架构组接受其作为
设计输入，不接受原始 `T1/T2` 排期和 `ARCH` 重叠率作为实现授权。

- **立即进入下一开发清债包**：W-06（UTF-8 截断 panic）、W-05（provider usage 事实接入，
  不引入通用 tokenizer 冒充真实 usage）、B-11/B-12 与小型仓内去重。它们可随下一个大开发
  滚动交付，但必须有回归测试和明确 owner。
- **独立迁移里程碑**：W-01..W-04。官方 Rust MCP SDK `rmcp` 是正确候选，但迁移同时改变
  protocol version、异步生命周期、schema 生成和 exact-14 对外形状，不能混入普通清债。
- **先架构、后开发**：W-07/W-09/W-10/W-12。它们都受同步 trait、runtime ownership、
  deadline/cancellation 语义约束；不能把 `tokio::spawn` 或新 codec 当一行替换。
- **只做 A/B 实验**：W-14/W-15/W-16。先固定相同数据、召回/图语义、持久化身份和资源预算，
  再决定替换；不因库声明支持某 metric 就推断产品语义等价。
- **暂不替换持久化真值簇**：W-17 与 ARCH。R4 前后都不得用 redb/SQLite 重写 v53 ledger。
  通用数据库只能候选替换底层事务介质，不能替代 CAS CID、域分离哈希、canonical revision、
  bounded read、fail-closed topology、typed `CommitIndeterminate` 与 replay receipt。

## 2. 为什么不能“全量立即替换”

1. `rmcp` 是官方 Rust SDK且支持 client/server、stdio、通知与任务，但当前主线已进入 3.x，
   protocol 与 Rust API仍在演进。迁移必须固定 SDK revision、MCP protocol version 与兼容 corpus，
   不能只做编译通过。
2. SQLite 提供成熟的原子事务和 WAL，但 WAL/SHM 是持久状态的一部分，checkpoint 与 sync failure
   有独立故障语义；这不是现有 immutable CAS + 双槽发布的 drop-in 等价物。
3. redb 是本仓已使用的纯 Rust、ACID、默认 crash-safe 嵌入式 KV，因而是最低供应链成本的
   storage experiment 候选；它仍不提供 Plico 的证据链、权限、canonical hash 与 receipt 语义。
4. Fjall 提供 single-writer/optimistic transactional 模式，但 durability 由调用方选择，默认写入
   OS buffer并不等于落盘；引入它会新增引擎、格式和迁移成本，当前优先级低于 redb 对照。

官方依据：

- RMCP official Rust SDK: https://github.com/modelcontextprotocol/rust-sdk
- SQLite atomic commit: https://www.sqlite.org/atomiccommit.html
- SQLite WAL: https://www.sqlite.org/wal.html
- redb: https://github.com/cberner/redb
- Fjall: https://github.com/fjall-rs/fjall

## 3. 统一替换准入门（不再 case-by-case）

每个 replacement package 必须同时满足：

1. **语义等价**：冻结输入、输出、错误分类、权限、default-off 与 exact public surface；
2. **存量可读**：给出旧格式读取/迁移/回滚方案，不静默重建 canonical truth；
3. **故障等价**：kill/crash/fsync/timeout/cancel/poison/reopen 反例不弱于旧实现；
4. **边界不扩张**：无新网络服务、无 silent remote fallback、CAS 仍是唯一宿主 FS owner；
5. **供应链可控**：固定版本、许可证、依赖树、离线构建、维护状态与安全公告可复核；
6. **净收益可证**：删除的领域无关代码、故障率、性能、内存、二进制体积和维护成本有 A/B 数据；
7. **可撤回**：先以 adapter/experiment 接入，未通过门禁不得删 reference implementation。

任何一项缺失，结论只能是 research candidate，不能叫“替换完成”。

## 4. 自然演化顺序与所有权

| 阶段 | Owner | 输出 | 是否授权写产品代码 |
|---|---|---|---|
| R4 WP3B.1 独立验收 | **Plico 架构组 + 安全/存储审计** | 独立 corpus、mutation、故障与真值裁决 | 仅验收，不扩功能 |
| W0 rolling hygiene | **开发组** | W-06/W-05/死代码/文档与小型去重 | R4 后按 exact scope 授权 |
| MCP SDK migration-A | **外包架构组** | rmcp 版本/协议/兼容/生命周期 ADR 与 corpus | 否 |
| MCP SDK migration-B | **开发组** | 按已冻结 ADR 迁移 client/server/schema | 是，限 MCP scope |
| Durable backend experiment | **外包架构组** | redb 对照原型、fault/性能/迁移报告 | research branch only |

当前北极星不是“自研代码越少越好”，而是：通用机制交给成熟库，Plico 独有的证据真值和
可验证经验语义保持窄、可审计、可迁移。

## 5. 本轮发现的治理修正

- 开发组的事实核验不能命名为“架构组全量接受”；已降级为 research input。
- `wp3b1_spec.json` 又写入本机绝对 `CARGO_TARGET_DIR`。R4 架构分支已前向改为 `<RUNNER_ROOT>`，
  并把 packet 检查提升为 machine-readable contract 共用的 portable-value validator；历史 bytes 不改写，
  新门禁不再序列化用户名、Home 或 checkout 路径。
- 现有 WP3B.1 candidate 的 `facade.rs`/测试文件较长属于可维护性 P2；300 行是 review trigger，
  不是语义 gate。R4 后按自然职责拆分，禁止仅为过线制造 `part1/part2`。
