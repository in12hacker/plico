# docs/plans/ — Milestone & roadmap drafts (Tier B)

Human-facing milestone plans and iteration notes. **Tier A** source of truth for module APIs remains `AGENTS.md` + `src/**/INDEX.md`.

| File | Milestone |
|------|-----------|
| [v0.5_intent_router_agent_enforcement.md](v0.5_intent_router_agent_enforcement.md) | v0.5 — Intent router, agent enforcement, messaging, kernel split |
| [v0.6_dogfood_closing_exec_fs.md](v0.6_dogfood_closing_exec_fs.md) | **下一迭代 v0.6（草案）** — Dogfood CRUD 自动化、SKILL 对齐、`cpu_time_quota`、Clippy 策略、FS 拆分 |
| [dogfood_cli_read_agent_gap.md](dogfood_cli_read_agent_gap.md) | Dogfood — `get`/`--agent` 根因与修复记录（收口工作见 v0.6） |

## 规则

- 计划文档使用 `docs/milestones/TEMPLATE.md` 模板
- 已完成的计划迁移到 `docs/milestones/` 并重命名为 `vXX-<name>.md`
- 每个计划结尾附 **落地对照** 段落，记录实现与原始验收标准的偏差
