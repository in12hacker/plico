# 里程碑索引

所有里程碑文档统一存放在 `docs/milestones/` 目录下。
新建里程碑请复制 `TEMPLATE.md` 模板。

## 里程碑列表

| 版本 | 名称 | 日期 | 状态 |
|------|------|------|------|
| [v34](v34-model-selection.md) | 模型选择 + 架构增强 + Ingest Pipeline | 2026-04 | ✅ 完成 |
| [v35](v35-retrieval-depth.md) | 检索深度 + 意图路由 + MMR 多样性 | 2026-04 | ✅ 完成 |
| [v41](v41-async-cognitive.md) | 异步认知共生体 | 2026-05 | ✅ 完成 |
| [v43](v43-extreme-recall.md) | Extreme Recall & Memory Fusion | 2026-05 | ✅ 完成 |
| [v46](v46-summary.md) | 性能优化 + 多跳推理 + 端到端质量 | 2026-05-13 | ✅ 完成 |
| [v47](v47-document-restructure.md) | 文档体系重构 | 2026-05-14 | ✅ 完成 |
| [v48](v48-cognitive-deepening.md) | 认知引擎深化 | 2026-05-14 | ✅ 完成 |
| [v49](v49-wasm-quality.md) | WASM Runtime & Quality Hardening | 2026-05-14 | ✅ 完成（T1-T10, T16；T11-T15 待后续迭代） |
| [v50](v50-memory-rearchitecture.md) | Memory Rearchitecture | 2026-05-14 | ✅ 完成 |
| [v51](v51-summary.md) | Search Quality & Scope Isolation | 2026-05-17 | ✅ 完成（SAS 15/20，embedding 稳定性已修复） |
| [v52](v52-trace-observability.md) | Trace Observability + Quality Fixes | 2026-05-18 | ✅ 完成 |
| [v53](v53-execution-observation-spine.md) | Execution Observation Ledger Core（Phase 1A；[开发交接](v53-developer-handoff.md)） | 2026-08-16 | R0 v2 freeze candidate / implementation not started |

## 模板

新建里程碑时使用 [TEMPLATE.md](TEMPLATE.md) 模板，包含：
- 背景与问题
- 方案设计
- 任务拆分（每个任务必须有验证标准）
- 质量门控标准
- 退化判定规则
- 风险评估
- 验收标准
- 版本快照（完成后填写）

## 规则

- **禁止** 将版本快照数据写入 CLAUDE.md — 统一写入本目录下的 `vXX-summary.md`
- **禁止** 在非 milestones 目录下创建里程碑文档
- **每个里程碑** 必须有对应的 summary 文件记录质量基线和 benchmark 结果
