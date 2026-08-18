# v53 R3 Acceptance：WP3A Read Facade Freeze

**日期**：2026-08-18
**状态**：GO

- architecture capability base：`eefe7d86f184a8504de2d803a5ed5110616c71b9`（WP3A.2-A）
- reader adaptation：`0fd606100e64725ab1b86a52037ed249e40952a6`（WP3A.2-B）
- R3 remediation / accepted candidate：`cbef2f3`（P2 ×2 收口：EOF diff-check、偏差文档归档）
- 冻结记录：`docs/milestones/v53-r3-wp3a-freeze.md`（scope + readonly surface + 十项 mutation corpus）
- formal result：定向 97/97（含冻结语料 22/22 与外部探针）；全库 lib `2153/8/2`
  （唯一失败集 = 既有 8 个 mcp 环境项，KNOWN_ISSUES B1，架构所有）

R3 按 R2 同法执行：umask 077 + `git archive` 隔离 checkout + 冻结语料 overlay + 逐 `--exact` 正式纪律。
十项独立验证（stamp 重哈希 / alternate genesis / 重复 Started·Terminal / Terminal 无 Started /
current-view 自洽重哈希〔外部探针〕/ fresh 零突变 / same-Arc 共存 / exchange 完整快照 /
malformed·symlink·mode 零修复 / 20,000·20,001 边界）全绿。顺序门禁：定向 → clippy（冻结树零警告）→
fmt → diff-check，全绿后才跑一次全库 lib。

R3 只接纳只读 reader 消费 existing-only capability；不宣称 append、receipt、producer admission、
权限或 kernel/scheduler/public API 接线（WP3B 域）。`cas/mod.rs` 便利 re-export 保留待架构裁决。
下一开发里程碑建议名：**WP3B.1 — Single-Writer Append + Idempotent Receipt**（窄切片：单写者 append、
单调 clock、稳定 receipt；不接 producer admission / 权限 / 外部执行副作用）。
