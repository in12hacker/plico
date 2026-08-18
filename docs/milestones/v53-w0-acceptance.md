# v53 W0 / W0.1 架构接纳

- 状态：**GO**
- 日期：2026-08-18
- W0 candidate：`b7b38d934c8bb67a0affa01d9c883630499fd766`
- W0.1 candidate：`f5002fdb4ede37a675b7040d90e46f18642195d5`
- 架构任务：`a6bfafc`（在接纳分支与 candidate 组合）

## 裁决

P0=0，P1=0。W0 rolling hygiene 接纳。

- UTF-8 截断统一复用 `safe_truncate`，定向复核 1/1；
- Ollama provider usage 优先于估算，缺字段才 fallback，定向复核 3/3；
- 三个误删的 crate-public 符号按原签名/行为恢复并 deprecated；外部视角 compile canary 1/1；
- benchmark 测试路径 Architecture Deviation 追认；
- dead benchmark metrics 与失实 INDEX 清理可接纳；
- candidate 报告的 fmt/clippy/full-lib `2150/0/2` 作为开发组 self-evidence；本次架构复核按
  成本控制只重跑受影响用例，不冒充第二次全量独立证明。

## 滚动债

- `D-USAGE-1`：embedding/CAS 仍只有明确标注的估算值；未来 provider telemetry 设计解决；
- temporal 文件头仍有少量历史措辞；不影响行为，滚入最近相关文档包；
- 统一 public API diff 门由外包架构组评估，最小 canary 已防止本次回归。

以上均有 owner，且与 W0 正确性独立，不阻断 GO。

## 下一阶段

自然下一里程碑为 [v54 MCP SDK Migration-A](v54-mcp-sdk-migration-a.md)，先交外包架构组，
不直接交开发组。Phase A 只冻结协议、SDK、生命周期与兼容 corpus；Plico 架构组接受后才开实现包。
