# docs/plans/ — Milestone & roadmap drafts (Tier B)

Human-facing milestone plans and iteration notes. **Tier A** source of truth for module APIs remains `AGENTS.md` + `src/**/INDEX.md`.

| File | Milestone |
|------|-----------|
| [v0.5_intent_router_agent_enforcement.md](v0.5_intent_router_agent_enforcement.md) | v0.5 — Intent router, agent enforcement, messaging, kernel split |
| [v0.6_dogfood_closing_exec_fs.md](v0.6_dogfood_closing_exec_fs.md) | **下一迭代 v0.6（草案）** — Dogfood CRUD 自动化、SKILL 对齐、`cpu_time_quota`、Clippy 策略、FS 拆分 |
| [dogfood_cli_read_agent_gap.md](dogfood_cli_read_agent_gap.md) | Dogfood — `get`/`--agent` 根因与修复记录（收口工作见 v0.6） |

## Conventions

- Plans may duplicate Cursor-local `.cursor/plans/*.plan.md` for Git visibility.
- End each plan with a short **落地对照** section when implementation diverges from original acceptance wording (avoid silent drift).
