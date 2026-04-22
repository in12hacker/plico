# Plico 第八节点设计文档
# 驾具 — 从"你来查"到"我给你"

**版本**: v2.0（✅ 全部完成 — 16 测试通过，含 AI thinking 对照补全）
**日期**: 2026-04-20
**灵魂依据**: `system-v2.md`（Soul 2.0）+ Harness Engineering 对齐
**阶段**: ✅ COMPLETED — 16 集成测试 + 32 MCP 单元测试全部通过
**前置**: 节点 6（闭合回路）✅ 完成 / 节点 7（代谢）设计完成
**验证方法**: 集成测试驱动开发（TDD）

---

## 0. 为什么跳过 Node 7 直接做 Node 8？

Node 7（代谢）的核心是 F-20 Real Embedding，需要引入 `ort` 外部依赖和模型文件。
Node 8（驾具）的 5 个特性**零外部依赖**，全部是 Plico 现有代码的扩展，且直接解决
AI Agent 的**第一痛点**——失忆和首次连接摩擦。

产出/投入比：Node 8 > Node 7。先做 Node 8，Node 7 作为后续迭代。

## 1. Harness Engineering 对齐分析

| Harness 原则 | Plico 当前 | Node 8 目标 |
|-------------|-----------|-----------|
| Level 1 指令（<60行） | ❌ 无消费者指令 | F-28 `plico://instructions` |
| 自描述接口 | ⚠️ `action="help"` 需 round-trip | F-31 ActionRegistry |
| Feedforward Guide | ⚠️ 只有 teaching errors（被动） | F-30 Smart Handover（主动） |
| Safety Rails | ⚠️ 仅权限 + 回收站 | F-32 只读/写确认/速率限制 |
| Content Discovery | ❌ Agent 不知道 Plico 有什么 | F-29 `plico://profile` |

## 2. 五个特性

### F-28: `plico://instructions` 消费者指令资源

MCP resource，在 Agent 首次连接时自动可用。<60 行纯文本。
教会消费 Agent：3 工具用途、action 列表、作用域模型、最佳模式。

### F-29: `plico://profile` 内容画像资源

MCP resource，零 round-trip 告诉 Agent "Plico 里有什么"。
返回：对象总数、标签分布、Agent 活跃度、KG 摘要。

### F-30: Smart Handover 协议

`session_start` 增加 `handover_mode` 参数。
不是"给你所有变化"（delta），而是"给你恢复工作所需的最少信息"。
组装逻辑：最近 checkpoint 摘要 + 关键变更 + 未完成任务 + 已知问题。

### F-31: 声明式 ActionRegistry

从硬编码 match arms 迁移到数据驱动 registry。
`plico(action="help")` 自动从 registry 生成，永远准确。
新增 action = 新增 registry 条目，不改 MCP tool schema。

### F-32: 安全护栏

- `PLICO_READ_ONLY=true` → 所有写操作返回错误
- 写操作返回 `confirmation_required` 标记（MCP elicitation 兼容）
- 可配置速率限制（默认 60 req/min）

### F-33: `plico://actions` 零 round-trip action 注册表

MCP resource，直接序列化 ActionRegistry 为 JSON。
Agent 首次连接时可通过 resource 获取完整 action 列表，无需 tool call round-trip。

### F-34: Lost in the Middle 优化

handover_mode 响应中字段按 LLM 注意力分布排序：
`handover`（开头） → `session_started`（中间） → 元数据 → `ok`（锚点）。
零成本信息重排，提高 LLM 对关键信息的捕获率。

### F-35: MCP Prompts 暴露

Skills 映射为 4 个 MCP prompt templates：
- `debug-issue` — 搜索相关记忆 + 因果链 + 提出解决方案
- `store-experience` — put + remember + 因果关联
- `project-review` — session_start + growth + delta 综述
- `handover` — 生成交接摘要

## 3. 实施优先级

```
F-28 (instructions) ✅ ← 最简单
  ↓
F-29 (profile+KG) ✅ ← 含 KG edge type 分布
  ↓
F-30 (handover+KG因果链) ✅ ← 最高 AI 价值
  ↓
F-31 (registry) ✅ → F-33 (plico://actions resource) ✅
  ↓
F-32 (safety: readonly+rate limit) ✅
  ↓
F-34 (Lost in the Middle) ✅ + F-35 (MCP Prompts) ✅
```

## 4. Soul 2.0 对齐

| 公理 | 影响 | 说明 |
|------|------|------|
| 1 Token 最稀缺 | +3% | F-30 Handover 把恢复上下文从 ~15000 token 降到 <2000 |
| 2 意图先于操作 | +2% | F-28 指令让 Agent 首次就知道如何表达意图 |
| 5 机制不是策略 | ✅ | Registry 是纯机制；Handover 组装由 OS 做，内容选择规则透明 |
| 7 主动先于被动 | +5% | F-28/F-29/F-30 全部是主动推送，不等 Agent 来问 |

---

## 5. 实施验证

| 特性 | 测试 | 状态 |
|------|------|------|
| F-28 instructions | `f28_instructions_resource_available_in_list` | ✅ |
| F-28 instructions | `f28_instructions_content_under_60_lines` | ✅ |
| F-29 profile | `f29_profile_returns_valid_json` | ✅ |
| F-29 profile | `f29_profile_reflects_stored_content` | ✅ |
| F-29 profile KG | `f29_profile_includes_kg_summary` | ✅ |
| F-30 handover | `f30_handover_session_start_returns_context` | ✅ |
| F-30 handover | `f30_handover_mode_compact_limits_tokens` | ✅ |
| F-31 registry | `f31_help_action_lists_all_registered_actions` | ✅ |
| F-32 safety | `f32_read_only_mode_blocks_writes` | ✅ |
| F-32 safety | `f32_read_only_allows_search` | ✅ |
| F-32 rate limit | `f32_rate_limit_blocks_excessive_requests` | ✅ |
| F-33 actions | `f33_actions_resource_returns_registry` | ✅ |
| F-34 LitM | `f34_handover_response_puts_handover_first` | ✅ |
| F-35 prompts | `f35_prompts_list_returns_templates` | ✅ |
| F-35 prompts | `f35_prompts_get_returns_messages` | ✅ |
| 全链路 E2E | `e2e_harness_full_lifecycle` | ✅ |

**附带改进**:
- `plico` 主工具新增 `put`/`get`/`help` action，减少工具数量切换开销
- MCP capabilities 声明 `prompts: {}` 启用 prompts 能力
- 原有 32 MCP 单元测试 + 2 E2E 测试全部通过，零回归

---

*文档版本 v2.0。8 个特性（F-28~F-35），16 个集成测试，零新外部依赖。*
*对照 `docs/ai thinking.md` 全部 5 个启发 + 3 个 AI 设计建议均已覆盖。*
