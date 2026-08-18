# v53 W0.1：公开 API 兼容性修复

- 状态：**READY FOR DEVELOPMENT / W0 尚未接纳**
- 日期：2026-08-18
- W0 candidate：`b7b38d934c8bb67a0affa01d9c883630499fd766`
- 执行方：**开发组**
- 验收方：**Plico 架构组**
- 非执行方：外包架构组无需修改本修复包

## 1. 裁决

W0 candidate 的 UTF-8 截断与 Ollama usage 定向行为通过，删除的 benchmark 内部死指标及
INDEX 修正没有发现产品真值风险；新增 `benchmarks/tests/test_metrics_estimation.py` 的
Architecture Deviation 予以追认，因为任务单明确要求 benchmark 定向测试而原 allowlist
遗漏 pytest 收集路径。

但当前 **NO-GO**：W0 任务单同时声明“不改公共 API 签名”，candidate 却删除了三个
crate-public 符号：

1. `plico::util::safe_range`；
2. `plico::temporal::TemporalRange::expanded`；
3. `plico::temporal::Granularity::HalfYear`。

仓内 `rg` 零引用只能证明主仓没有调用，不能证明外部 Rust consumer 没有依赖。根因是
“dead-code 清理”缺少 public-surface 判定，而不是三个孤立 case。

## 2. 开发组任务

从 `b7b38d9` 创建一个窄 remediation commit：

1. 按 `7148f4d` 的签名和行为恢复上述三个公开符号；
2. 给三个符号增加 `#[deprecated]`，说明当前主线不再内部使用，但在下一次明确的 semver/
   public-API 变更前保留兼容性；不得改变其输入、返回值或枚举编码；
3. 新增一个外部视角 compile canary（integration test），通过 `plico::...` 路径实际引用
   三个符号，防止以后再次用仓内零引用误删；
4. 修正 `src/temporal/mod.rs` 的中置信度表述：当前消费者不会自动调用 `expanded`，
   不得继续宣称自动 ±7 天；保留 deprecated helper 不等于启用该行为；
5. 将 W0 delivery 中 B-10 标为“deprecated compatibility retained”，W-05 标为
   “Ollama真实 usage 已闭合；embedding/CAS estimator 仍为显式 fallback”。

## 3. Exact scope

允许修改：

```
src/util.rs
src/temporal/resolver.rs
src/temporal/rules.rs
src/temporal/mod.rs
tests/public_api_compat.rs
docs/milestones/v53-w0-delivery.md
docs/milestones/v53-w0-r1-remediation.md
docs/milestones/INDEX.md
```

其余文件零修改；不得借机改变 temporal 行为、token 估算、Cargo 依赖或 MCP。

## 4. 低成本验收顺序

1. exact diff + `git diff --check`；
2. compile canary，确认三个 public path 可用且只有 deprecation warning；
3. temporal/util 定向测试；
4. intent UTF-8 与 Ollama usage 定向回归；
5. 1–4 全绿后才复用 W0 已有 clippy/full-lib 证据；若源码只恢复原实现，可不重复 benchmark
   全套，只运行新增/受影响用例。

P0 或新增行为 P1 立即停止；纯文案/测试命名问题可登记后滚入下一大开发。

## 5. 后续统一工程门

本修复只加最小 canary。由**外包架构组**在后续开发前评估并冻结统一 public API diff 门
（优先考察离线固定版本的 `cargo-semver-checks` 或等价 rustdoc surface 比较），开发组不得
自行引入新工具依赖。以后“死代码”只有在 private、明确破坏性版本或通过该门时才能删除。

## 6. W0.1 之后

W0.1 接纳后，自然下一里程碑是 **MCP SDK Migration-A**，交给**外包架构组**：固定官方
`rmcp` 版本、MCP protocol version、stdio lifecycle、request/response ID、notification、deadline、
exact-14/schema 兼容 corpus。架构合同冻结后才交开发组实现；不得把迁移塞入 W0.1。

另外登记 `D-USAGE-1`：embedding/CAS 当前只有估算值，无 provider usage 来源。它是未来
provider telemetry 设计项，不把估算值冒充真实值，也不阻断本次 W0.1。
