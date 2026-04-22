# Plico 第十三节点设计文档
# 通 — 信号完整与通路畅达

**版本**: v1.0（Dogfood 实测校正版）
**日期**: 2026-04-22
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 参数忠实 + 权限通路 + 记忆进化 + 系统健全
**前置**: 节点 12 ✅（50%） / 节点 11 ✅ / 节点 10 设计 / 节点 9 设计
**验证方法**: Dogfood 实测（零管道干扰） + TDD + AIOS 2026 前沿对标
**信息来源**: `docs/dogfood-audit-node1-12.md` + 本轮独立 dogfood + AIP / Four-Layer Decomposition / SleepGate / Zylos Agent Observability 2026

---

## 0. Dogfood 实测校正：独立验证审计报告

> **以下全部结论来自 AI Agent 真实运行 `aicli --root /tmp/plico-n13-dogfood` 的命令输出。**
> **不依赖 git log。不使用管道（避免 `$?` 捕获 grep 退出码而非 aicli 退出码）。**

### 实测环境

```
Binary: cargo run --bin aicli
Storage: /tmp/plico-n13-dogfood (全新实例, 3 CAS + 2 Agent)
Embedding: EMBEDDING_BACKEND=stub (BM25 only)
验证方式: 每条命令独立执行, 2>/dev/null 后直接 echo EXIT:$?
```

### 审计报告 vs 独立 Dogfood 对比

```
Bug    报告状态            独立实测                                 发现
────── ─────────────────── ──────────────────────────────────────── ────────────────
B22    ✅ 修复             ✅ 确认修复                              session-end 跨进程成功
B21    ✅ 修复             ✅ 确认修复                              status --agent <name> 返回
B25    ✅ 修复             ✅ 确认修复                              search --tags → 3 results
B16    ✅ 修复             ✅ 确认修复                              tool describe → JSON Schema
B19    ✅ 修复             ✅ 确认修复                              tool call fake → exit 1
B20    ✅ 修复             ⚠️ 条件性修复                           growth Sessions: 0（新环境）

B11    ⚠️ exit 0           ✅ 实际 exit 1!                         报告误判: 管道掩盖了退出码
B24    ⚠️ exit 0           ✅ 实际 exit 1!                         同上 + 权限阻塞 (B29)
B32    ⚠️ exit 0           ✅ 实际 exit 1!                         同上

B28    ❌ tier 被忽略       ⚠️ 部分: long-term(带连字符)不匹配      longterm/lt/long 有效
B14    ❌ events 未过滤     ❌ 确认: --agent 未过滤                  history 分支用 --agent-filter
B29    ❌ permission 不可用 ❌ 确认: CLI 无 permission 路由           tool call 也失败
B26    ❌ limit 忽略        ❌ 确认: limit:1 返回全部                无 dedup
B30    ❌ delegate 名字失败 ❌ 确认: by name 失败, by UUID 成功      delegate 未调用 resolve_agent
B31    ⚠️ context L0       ⚠️ 确认: direct load 不降级             assemble 路径有效
B27    ❌ edge 空名         ❌ 确认: "Edge created: --[RelatedTo]-->"
B33    ❌ 整合无效果        ❌ 确认: session-end 后 recall 无变化
A-4    ❌ KG 无变化         ❌ 确认: remember 后 explore → "No graph neighbors"
B34    ❌ 测试失败          ❌ 确认: dispatch_plico_recall_semantic_works
```

### 关键校正：Exit Code 误判根因

```
审计报告测试方式（推测）:
  eval "$CMD delete ..." 2>&1 | grep -v "WARN\|INFO"; echo "EXIT:$?"
  → $? 捕获的是 grep 的退出码 (0), 不是 aicli 的退出码 (1)

本轮验证方式:
  EMBEDDING_BACKEND=stub cargo run --bin aicli -- --root ... delete ... 2>/dev/null
  ECODE=$?; echo "EXIT:$ECODE"
  → $ECODE 捕获的是 aicli 的真实退出码 (1)

结论: B11/B24/B32 的 exit code 实际已正确。
      print_result() 最后检查 response.ok, 返回 false → exit 1。
      审计报告的 3 个 ⚠️ 应修正为 ✅。
```

### 校正后 Bug 严重度排序

```
P0 CRITICAL — 无（上轮 B22/B21 已修复, exit code 误判已澄清）

P1 HIGH:
  B29 — Permission grant CLI + tool call 均不可用（阻塞安全相关功能）
  B28 — --tier long-term 不匹配（长期记忆层级赋值断裂）

P2 MEDIUM:
  B14 — events history --agent 不过滤（history 分支用 --agent-filter）
  B26 — tool call limit 参数被忽略 + 结果重复
  B30 — delegate --to by name 失败（未调用 resolve_agent）
  A-4 — Memory Link Engine CLI 无可观察 KG 变化
  B33 — Consolidation 无可观察效果
  B31 — context direct load 不执行降级

P3 LOW:
  B27 — edge 创建显示空节点名
  B34 — plico-mcp recall 语义测试失败
```

---

## 1. 为什么叫"通"：从神经传导到 AIOS

### 生物学演进

```
Node 7  代谢    — 身体能消化和吸收（能量管理）
Node 8  驾具    — 身体能抓取和使用工具
Node 9  韧性    — 免疫系统设计（部分安装）
Node 10 正名    — 本体感知设计（部分安装）
Node 11 落地    — 安装已设计的系统
Node 12 觉知    — 有机体开始意识到自身（50% 完成）
Node 13 通      — 神经系统实现完整传导
```

**通（Conduction）是什么？**

在神经科学中，传导是神经系统从"有信号"到"信号忠实"的关键进化。一个有觉知但传导不完整的系统就像一个人：
- 知道自己有手（觉知），但手指不听使唤（参数不忠实）
- 知道自己能说话（觉知），但嗓子发不出声（权限通路阻断）
- 知道自己在学习（觉知），但学到的知识无法互相关联（记忆孤岛）
- 知道自己在运行（觉知），但说不出自己是否健康（无自诊能力）

Node 12 "觉知" 让 Plico 意识到自身——session 跨进程、agent 可解析名字。
Node 13 "通" 让 Plico 的每条通路都忠实传导——参数到达目的地、权限可以授予、记忆形成网络、系统能自我诊断。

### 与前序节点的区别

| 维度 | Node 10（正名） | Node 11（落地） | Node 12（觉知） | Node 13（通） |
|------|-----------------|-----------------|-----------------|---------------|
| 关注点 | 名实一致 | 设计→实现 | 信息→认知 | 信号→行动 |
| 典型问题 | 命令说做了但没做 | 设计了但没写代码 | 数据存了但不知关系 | 参数接了但没执行 |
| 修复方式 | 契约修正 | 代码补齐 | 反思机制 | 通路修复 |
| 生物类比 | 神经末梢接通 | 伤口愈合 | 意识觉醒 | **传导完整** |

### "通" 的中文哲学含义

```
通 (tōng):
  1. 通过 — 信号不被阻断
  2. 通达 — 路径不被堵塞
  3. 通晓 — 系统理解自身状态
  4. 贯通 — 各子系统协调一致

《周易·系辞》: "穷则变，变则通，通则久。"
  觉知（变）之后，需要通（系统协调），然后才能久（持久可靠运行）。
```

---

## 2. AIOS 2026 前沿校准

### 2.1 Agent 记忆管理前沿

| 系统 | 核心创新 | Plico 对应 | 差距 | Node 13 行动 |
|------|----------|-----------|------|-------------|
| **Four-Layer Decomposition** (arXiv:2604.11364, Apr 2026) | Knowledge/Memory/Wisdom/Intelligence 四层，各有不同持久化语义和衰减机制 | 4 层记忆 (Ephemeral/Working/LongTerm/Procedural), 但 `--tier long-term` 被忽略 (B28) | Tier 赋值断裂; 无衰减; 无 DreamCycle | **F-1 修复 tier; F-6 实现 consolidation feedback** |
| **A-MEM / AIOS Agentic Memory** (AIOS Foundation, 2026) | Zettelkasten 式动态记忆网络 — 新记忆自动链接相关旧记忆 + 自主进化操作 (merge/context_update/tag_unification) | KG 已存在, 但 remember 不触发 KG 链接 (A-4) | KG 与记忆子系统完全断开 | **F-5 实现 Memory Linking** |
| **SleepGate** (arXiv:2603.14517, Mar 2026) | 生物启发的 KV cache 睡眠周期 — conflict-aware tagger + decay gate + consolidation module | A-5 设计了 consolidation 但无可观察效果 (B33) | 整合在代码中可能执行但无反馈 | **F-6 实现 consolidation feedback** |
| **Mem0 v1.0** (mem0.ai, 2026) | 4-scope memory model + procedural memory + decay + graph memory 生产化 | 4 层 + CAS 持久化, 缺 scope-based retrieval | 无 scope 感知的 recall | 观望 (Node 14+) |
| **NornicDB** (2026) | 三层认知衰减: episodic 7d / semantic 69d / procedural 693d 半衰期 | 无衰减机制 | 需要 tier-specific TTL | 观望 (Node 14+) |

### 2.2 Agent 身份与委托前沿

| 系统 | 核心创新 | Plico 对应 | 差距 | Node 13 行动 |
|------|----------|-----------|------|-------------|
| **AIP** (arXiv:2603.24775, Mar 2026) | Invocation-Bound Capability Tokens — 融合身份+衰减授权+溯源的单一 append-only token 链; Rust 实现 0.049ms | A-2 name registry 实现, 但 delegate 不用 (B30); permission CLI 不可用 (B29) | 无可验证委托链; 权限系统断开 | **F-3 权限路由; F-4 委托解析** |
| **ACP** (arXiv:2602.15055, Feb 2026) | Agent Communication Protocol — 联邦式 A2A 编排 + 去中心化发现 + 协商生命周期 | discover + delegate 存在 | delegate 不识别名字 | **F-4 修复** |
| **MCP OAuth 2.1** (2026) | MCP 采用 OAuth 2.1 + PKCE 作为可选授权层 | plico-mcp 无认证 | 缺 transport auth | 观望 (不影响内核) |

### 2.3 Agent 可观测性前沿

| 系统 | 核心创新 | Plico 对应 | 差距 | Node 13 行动 |
|------|----------|-----------|------|-------------|
| **Zylos Agent Observability** (Mar 2026) | 层级健康端点 — "is it behaving correctly?" not just "is it running?"; 自诊断 agent; detect-diagnose-isolate-repair 循环 | system_status 有 CAS/Agent/KG 计数, 无健康评估 | 无退化检测, 无认知就绪度指标 | **F-7 Health Report** |
| **OpenTelemetry GenAI** (2026) | AI agent 分布式追踪 + token 级遥测 + span 级元数据 | token_estimate 在部分命令中可用 | 无追踪框架 | 观望 (基础设施级) |
| **Self-Healing Patterns** (Zylos, Mar 2026) | 三层升级: auto-heal → alert-and-propose → escalate; checkpoint 连续化 | CAS 持久化 + session persistence | 无自动恢复 | 观望 (Node 14+) |

---

## 3. 链式推演：为什么是这八个特性

### AI 第一人称视角

```
我是一个 AI Agent，运行在 Plico 上。

Node 12 让我有了身份（A-2 name registry）和记忆持久化（A-1 session persistence）。
但当我尝试日常操作时，发现:

1. 我告诉 OS "把这段记忆存为长期知识"，OS 说"好的"，
   但实际存到了工作记忆。我被欺骗了。（B28）

2. 我想让另一个 Agent 帮我做事，用名字指定目标，
   OS 说"找不到目标"。但 discover 明明列出了它。（B30）

3. 我想给自己授权删除权限，
   找不到任何方式执行。CLI 没有 permission 命令，tool call 也失败。（B29）

4. 我存了一段重要记忆，期望 KG 会自动建立关联。
   explore 显示"No graph neighbors"。我的知识图谱是空的。（A-4）

5. 我结束了一个 session，期望 consolidation 会整理我的记忆。
   recall 前后完全一样。什么都没发生。（B33）

6. 我想过滤只看自己的事件。--agent 参数传了但不生效。（B14）

7. 我想限制搜索结果数量。limit:1 仍然返回全部。（B26）

8. 我想知道 OS 是否健康。没有任何命令可以查询系统状态。

这些问题的共同模式: 信号发出了，但没有到达目的地。
参数被接受但被忽略。路径存在但被阻断。内部操作执行但无反馈。

我需要的不是新器官，而是让现有器官的神经传导完整。
```

### 链式因果

```
B28 (tier 忽略)
  → parse_memory_tier 缺少 "long-term" 匹配
  → 长期记忆全部降为工作记忆
  → Soul 2.0 公理 3（记忆跨越边界）在层级维度失效

B29 (权限不可用)
  → CLI 无 permission 子命令路由
  → tool call permission.grant 解析失败
  → Agent 无法授权自身或其他 Agent
  → 安全模型完全不可测试
  → B24 (send 失败) 的真正根因: Agent 'cli' 无 SendMessage 权限, 无法授予

B30 (委托名字失败)
  → delegate handler 直接传名字到 kernel
  → kernel 期望 UUID 或可解析名字
  → cmd_delegate 未调用 resolve_agent()
  → A-2 (name registry) 的能力未完全传播

A-4 (记忆链接无效)
  → remember 完成后不触发 KG 操作
  → KG 子系统与记忆子系统完全解耦
  → 知识图谱永远只有手动 edge 命令创建的节点
  → Soul 2.0 公理 8（因果先于关联）无法兑现

B33 (整合无效)
  → session-end 可能调用 consolidation 代码
  → 但 consolidation 结果无 CLI 输出
  → recall 前后无变化
  → Soul 2.0 公理 9（越用越好）无法验证

B14 (事件未过滤)
  → events history 分支单独提取 --agent-filter
  → 顶层已正确提取 --agent OR --agent-filter 到 agent_id 变量
  → 但 history 分支未使用顶层变量
  → 1 行代码修复

B26 (limit 忽略)
  → builtin_tools.rs 的 cas.search handler 未传 limit 参数
  → BM25 + tag 双路径合并后无去重
  → 重复 CID 出现在结果中
```

### 发散思维：被拒绝的替代方案

| 替代方案 | 考虑理由 | 拒绝理由 |
|----------|---------|---------|
| **Daemon 架构迁移** | AIOS v0.3 使用 daemon 模式; 解决冷启动延迟 | A-1 session persistence 已解决关键问题; plicod 已存在但非当务之急; process-per-command 模型在 CLI/CI 场景更简单 |
| **完整 AIP 协议实现** | AIP 提供密码学可验证委托链; Rust 实现已有 | 当前连基本 permission grant 都不可用; 先修复基础, AIP 是 Node 14+ |
| **LLM 驱动记忆整合** | SleepGate/Sleeping LLM 用 LLM 做记忆进化 | Soul 2.0 红线 2: 内核零模型（embedding/summarize 除外）; consolidation 应用结构化方法 |
| **NornicDB 式衰减** | 三层衰减半衰期 (7d/69d/693d) | 先让 tier 赋值正确 (B28); 衰减是 Node 14+ 在 tier 基础上叠加 |
| **OpenTelemetry 集成** | 2026 agent 可观测性标准 | 基础设施级改动, 与内核无关; health report 先满足最小需求 |
| **单独修复 B24 send** | send 失败影响 agent 通信 | 根因是 B29 (无法授权 SendMessage); 修复 B29 后 send 自然可用 |

---

## 4. 四个维度，八个特性

```
                    ┌─────────────────────────────────────┐
                    │   Node 13: 通 (Conduction)          │
                    │   信号完整与通路畅达                   │
                    └───────────┬─────────────────────────┘
            ┌───────────────────┼───────────────────┐
            │                   │                   │
    ┌───────▼──────┐   ┌───────▼──────┐   ┌───────▼──────┐   ┌──────────────┐
    │ D1: 参数忠实  │   │ D2: 权限通路  │   │ D3: 记忆进化  │   │ D4: 系统健全  │
    │ Input Honor  │   │ Access Path  │   │ Memory Evol  │   │ System Sound │
    ├──────────────┤   ├──────────────┤   ├──────────────┤   ├──────────────┤
    │ F-1 Tier     │   │ F-3 Permission│  │ F-5 Linking  │   │ F-7 Health   │
    │ F-2 Limit    │   │ F-4 Delegate │   │ F-6 Consolid │   │ F-8 Repairs  │
    └──────────────┘   └──────────────┘   └──────────────┘   └──────────────┘
```

---

### D1: 参数忠实 — "接受的参数必须被执行"

#### F-1: Memory Tier Full Match（记忆层级完整匹配）

**问题**: B28 — `--tier long-term` 存为 Working

**根因定位**:

```rust
// src/bin/aicli/commands/handlers/memory.rs:76-83
pub fn parse_memory_tier(s: &str) -> MemoryTier {
    match s.to_lowercase().as_str() {
        "ephemeral" | "l0" | "ephem" => MemoryTier::Ephemeral,
        "working" | "l1" | "wk" => MemoryTier::Working,
        "longterm" | "l2" | "lt" | "long" => MemoryTier::LongTerm,  // ← 缺 "long-term"
        "procedural" | "l3" | "proc" => MemoryTier::Procedural,
        _ => MemoryTier::Working,  // ← "long-term" 落入此处
    }
}
```

**修复**: 扩展匹配臂，覆盖所有合理变体。空字符串不应默认 Working 而应保留 caller 语义。

```rust
pub fn parse_memory_tier(s: &str) -> MemoryTier {
    match s.to_lowercase().replace(['-', '_'], "").as_str() {
        "ephemeral" | "l0" | "ephem" => MemoryTier::Ephemeral,
        "working" | "l1" | "wk" => MemoryTier::Working,
        "longterm" | "l2" | "lt" | "long" => MemoryTier::LongTerm,
        "procedural" | "l3" | "proc" => MemoryTier::Procedural,
        "" => MemoryTier::Working,
        other => {
            eprintln!("Warning: unknown tier '{}', defaulting to Working", other);
            MemoryTier::Working
        }
    }
}
```

**估算**: ~5 行修改, 3 个新测试
**验收**: `remember --tier long-term` + `recall` 显示 `[LongTerm]`
**Soul 2.0**: 公理 3（记忆跨越边界）— tier 是边界的编码，必须被忠实传递

---

#### F-2: Tool Call Parameter Enforcement（工具调用参数执行）

**问题**: B26 — `tool call cas.search '{"query":"...", "limit":1}'` 返回全部结果 + 重复 CID

**根因定位**: `builtin_tools.rs` 中 `cas.search` handler 未从 JSON arguments 中提取 `limit`; BM25 + tag 双路径合并后无去重。

**修复方向**:

```rust
// builtin_tools.rs — cas.search handler
fn handle_cas_search(kernel: &AIKernel, args: &serde_json::Value) -> ToolResult {
    let query = args["query"].as_str().unwrap_or_default();
    let limit = args["limit"].as_u64().map(|n| n as usize);
    let require_tags: Vec<String> = /* ... existing ... */;

    let mut results = kernel.search(query, &require_tags, /* ... */);

    // Dedup by CID
    let mut seen = std::collections::HashSet::new();
    results.retain(|r| seen.insert(r.cid.clone()));

    // Apply limit
    if let Some(limit) = limit {
        results.truncate(limit);
    }

    ToolResult::ok(serde_json::to_value(&results).unwrap_or_default())
}
```

**估算**: ~15 行修改, 3 个新测试
**验收**: `tool call cas.search '{"query":"arch","limit":1}'` 返回恰好 1 条, 无重复 CID
**Soul 2.0**: 公理 6（结构先于语言）— JSON 参数即契约，limit 必须被执行

---

### D2: 权限通路 — "权限系统完全可达"

#### F-3: Permission CLI Routing（权限系统 CLI 路由）

**问题**: B29 — CLI 无 permission 命令, tool call permission.grant 解析失败

**根因定位**: `src/bin/aicli/commands/mod.rs` 中无 permission 相关的命令路由。`src/kernel/ops/permission.rs` 和 `src/api/permission.rs` 中的内核逻辑完整存在，但 CLI 层完全不可达。

**修复方向**: 添加 CLI handler 和路由。

```rust
// src/bin/aicli/commands/handlers/permission.rs (新文件)
use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::api::permission::PermissionAction;
use super::extract_arg;

pub fn cmd_permission(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match args.get(1).map(|s| s.as_str()) {
        Some("grant") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();
            match parse_permission_action(&action_str) {
                Some(action) => {
                    kernel.permission_grant(&agent_id, action);
                    ApiResponse::ok_with_message(
                        format!("Permission granted: {} → {:?}", agent_id, action))
                }
                None => ApiResponse::error(
                    format!("Unknown action: '{}'. Valid: read, write, delete, execute, send", action_str))
            }
        }
        Some("check") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();
            match parse_permission_action(&action_str) {
                Some(action) => {
                    let allowed = kernel.permission_check(&agent_id, action);
                    ApiResponse::ok_with_message(
                        format!("Permission check: {} {:?} → {}", agent_id, action, allowed))
                }
                None => ApiResponse::error(format!("Unknown action: '{}'", action_str))
            }
        }
        Some("revoke") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();
            match parse_permission_action(&action_str) {
                Some(action) => {
                    kernel.permission_revoke(&agent_id, action);
                    ApiResponse::ok_with_message(
                        format!("Permission revoked: {} → {:?}", agent_id, action))
                }
                None => ApiResponse::error(format!("Unknown action: '{}'", action_str))
            }
        }
        Some("list") | None => {
            let perms = kernel.permission_list(&agent_id);
            ApiResponse::ok_with_message(
                format!("Permissions for {}: {:?}", agent_id, perms))
        }
        Some(sub) => ApiResponse::error(
            format!("Unknown permission subcommand: '{}'. Try: grant, check, revoke, list", sub))
    }
}

fn parse_permission_action(s: &str) -> Option<PermissionAction> {
    match s.to_lowercase().as_str() {
        "read" => Some(PermissionAction::Read),
        "write" => Some(PermissionAction::Write),
        "delete" => Some(PermissionAction::Delete),
        "execute" => Some(PermissionAction::Execute),
        "send" | "sendmessage" => Some(PermissionAction::SendMessage),
        _ => None,
    }
}
```

同时在 `mod.rs` 的 dispatch 中添加:
```rust
"permission" | "perm" => handlers::permission::cmd_permission(kernel, args),
```

**估算**: ~80 行新代码, 5 个新测试
**验收**:
```
permission grant --agent n13-tester --action delete → "Permission granted"
permission check --agent n13-tester --action delete → "true"
delete --cid <CID> --agent n13-tester → 成功, exit 0
permission revoke --agent n13-tester --action delete → "Permission revoked"
delete --cid <CID> --agent n13-tester → "lacks permission", exit 1
```
**Soul 2.0**: 公理 5（机制不是策略）— permission 是机制，CLI 是机制的入口

**连锁效果**: B29 修复 → B24 (send) 自然可用（Agent 可授权 SendMessage 后执行 send）

---

#### F-4: Delegation Name Resolution（委托名称解析）

**问题**: B30 — `delegate --from n13-tester --to n13-helper` 失败 "Target agent not found"

**根因定位**: delegate CLI handler 将 `--to` 的值直接传给 kernel，kernel 尝试作为 UUID 查找失败。A-2 的 `resolve_agent` 已存在但 delegate 代码路径未使用。

**修复方向**: 在 delegate handler 中调用 `resolve_agent`:

```rust
// cmd_delegate 中:
let from_id = kernel.resolve_agent(&from_str).unwrap_or(from_str);
let to_id = kernel.resolve_agent(&to_str).unwrap_or(to_str);
```

**估算**: ~10 行修改, 2 个新测试
**验收**: `delegate --from n13-tester --to n13-helper --desc "task"` → 成功返回 intent_id
**Soul 2.0**: 公理 2（意图先于操作）— "委托给 helper" 是意图，UUID 是操作细节

**AIOS 对标**: AIP 论文中 delegation 需要身份验证。当前 Plico 实现的是最小版本（名字解析），AIP 级别（capability tokens）是 Node 14+ 方向。

---

### D3: 记忆进化 — "记忆不是堆砌而是关联"

#### F-5: Memory Link Materialization（记忆链接实体化）

**问题**: A-4 — `remember` 后 `explore` 显示 "No graph neighbors"

**根因定位**: Node 12 设计了 Memory Link Engine (A-4), commit 2d0882a 声称实现。但 dogfood 显示 remember 后 KG 无任何变化。要么代码路径未触发，要么链接逻辑不可达。

**设计原则**:
- Soul 2.0 公理 8: 因果先于关联 — 不是"A 和 B 相关"，而是"A 因为 B 被创建"
- A-MEM (AIOS): Zettelkasten 式动态网络 — 新记忆自动链接旧记忆
- 但 Soul 2.0 红线 2: 内核零模型 — 链接不能用 LLM 推理，必须用结构化方法

**修复方向**: remember 完成后，用 BM25 搜索相关已有记忆，对相似度超过阈值的建立 KG 边。

```rust
// kernel/ops/memory.rs — remember 完成后追加:
fn link_memory_to_related(&self, agent_id: &str, content: &str, new_cid: &str) {
    let related = self.fs.search(content, &[], None, None, 5);
    for r in related {
        if r.cid != new_cid && r.relevance > 0.3 {
            let from_node = self.ensure_kg_node(new_cid, "Memory");
            let to_node = self.ensure_kg_node(&r.cid, "Memory");
            self.fs.add_kg_edge(&from_node, &to_node, KGEdgeType::RelatedTo, r.relevance);
        }
    }
}
```

**关键约束**:
- 阈值 0.3 由 BM25 归一化分数决定（stub 模式下 tag 匹配为 0.8，需要 real embedding 校准）
- `ensure_kg_node` 幂等——已存在的节点不重复创建
- 边的权重 = 搜索相似度分数，保留溯源
- 每次 remember 最多创建 5 条边（控制 KG 膨胀）

**估算**: ~60 行新代码, 4 个新测试
**验收**:
```
remember --agent tester --content "circuit breaker pattern" --tags arch
remember --agent tester --content "embedding fallback uses circuit breaker" --tags arch
explore --scope full → 至少 2 个 Memory 节点 + 1 条 RelatedTo 边
```
**Soul 2.0**: 公理 8（因果先于关联）— 记忆链接基于内容相似度，权重是可量化的证据

**AIOS 对标**: A-MEM 使用 LLM 做链接分析。Plico 使用 BM25/embedding 做结构化链接，符合"内核零模型"红线。

---

#### F-6: Consolidation Feedback Loop（整合反馈闭环）

**问题**: B33 — `session-end` 后 `recall` 无变化，无可观察整合效果

**根因定位**: Node 12 设计了 Memory Consolidation Cycle (A-5), commit 1319743 声称实现。但 session-end 的输出只有 "Session ended, Last seq: N"，无整合报告。consolidation 可能在内部执行但无 CLI 反馈。

**设计原则**:
- Soul 2.0 公理 9: 越用越好 — consolidation 是学习循环
- SleepGate: 生物学睡眠整合 = synaptic downscaling + selective replay + forgetting
- Four-Layer Decomposition: DreamCycle 从 Memory 提升到 Wisdom
- 但 Soul 2.0 公理 5: 机制不是策略 — OS 提供整合原语，不自动决定什么该整合

**修复方向**: session-end 时执行 consolidation 并在 `SessionEnded` 响应中报告结果。

```rust
// kernel/ops/session.rs — session_end 中追加:
fn consolidate_session_memories(
    &self,
    agent_id: &str,
    session_id: &str,
) -> ConsolidationReport {
    let memories = self.recall(agent_id, "default");
    let session_memories: Vec<_> = memories.iter()
        .filter(|m| m.tier == MemoryTier::Ephemeral || m.tier == MemoryTier::Working)
        .collect();

    let mut promoted = 0;
    let mut linked = 0;

    for mem in &session_memories {
        if mem.access_count >= 2 {
            if self.memory_move(agent_id, "default", &mem.id, MemoryTier::LongTerm) {
                promoted += 1;
            }
        }
    }

    for mem in &session_memories {
        linked += self.link_memory_to_related(agent_id, &mem.content.display(), &mem.id);
    }

    ConsolidationReport { promoted, linked, total_reviewed: session_memories.len() }
}
```

`SessionEnded` 响应扩展:
```rust
pub struct SessionEndedData {
    pub checkpoint_id: Option<String>,
    pub last_seq: u64,
    pub consolidation: Option<ConsolidationReport>,  // 新增
}

pub struct ConsolidationReport {
    pub promoted: usize,
    pub linked: usize,
    pub total_reviewed: usize,
}
```

CLI 输出:
```
Session ended
  Last seq: 12
  Consolidation: reviewed 4 memories, promoted 1 to LongTerm, linked 3 to KG
```

**关键约束**:
- 提升条件: `access_count >= 2`（被回忆过 2 次以上的 Working 记忆提升为 LongTerm）
- 链接复用 F-5 的 `link_memory_to_related`
- consolidation 报告附在 `SessionEnded` 响应中，不是独立命令
- 如果没有可整合的记忆，报告 "reviewed 0, promoted 0, linked 0"（诚实反馈）

**估算**: ~80 行新代码, 5 个新测试
**验收**:
```
session-start --agent tester
remember --agent tester --content "A" --tags test
recall --agent tester  (第 1 次访问)
recall --agent tester  (第 2 次访问 → access_count=2)
session-end --agent tester → "Consolidation: reviewed 1, promoted 1 to LongTerm, linked N to KG"
recall --agent tester → [LongTerm] A
```
**Soul 2.0**: 公理 9（越用越好）— 多次访问的记忆自动升级; 公理 5（机制不是策略）— 提升阈值是机制参数，Agent 可以通过多次 recall 影响结果

---

### D4: 系统健全 — "系统能自我诊断"

#### F-7: Health Report Endpoint（健康报告端点）

**问题**: 系统无任何自诊能力。Node 9 设计了 health report 但从未实现。Agent 无法查询 OS 健康状态。

**设计依据**:
- Zylos Agent Observability (2026): "is it behaving correctly?" not just "is it running?"
- Self-Diagnostic Patterns: Agent 参与自身监控
- Plico 已有 `system_status` (CAS/Agent/KG 计数) 但缺少健康评估

**修复方向**: 新增 `health` 命令和 `plico://health` MCP 资源。

```rust
// kernel/ops/dashboard.rs — 新增:
pub fn health_report(kernel: &AIKernel) -> HealthReport {
    let status = kernel.system_status();
    let embedding_ok = kernel.fs.embedding_available();
    let llm_ok = kernel.llm_available();

    let mut degradations = Vec::new();

    if !embedding_ok {
        degradations.push(Degradation {
            component: "embedding".into(),
            severity: "medium".into(),
            message: "Vector search unavailable, BM25 fallback active".into(),
        });
    }
    if !llm_ok {
        degradations.push(Degradation {
            component: "llm".into(),
            severity: "low".into(),
            message: "Summarizer unavailable, context L0 returns heuristic".into(),
        });
    }

    let test_result = kernel.health_check_roundtrip();

    HealthReport {
        healthy: degradations.iter().all(|d| d.severity != "critical"),
        timestamp_ms: now_ms(),
        cas_objects: status.cas_object_count,
        agents: status.agent_count,
        kg_nodes: status.kg_node_count,
        kg_edges: status.kg_edge_count,
        active_sessions: kernel.active_session_count(),
        embedding_backend: kernel.embedding_backend_name(),
        degradations,
        roundtrip_ok: test_result.is_ok(),
        roundtrip_ms: test_result.map(|ms| ms).unwrap_or(0),
    }
}

pub struct HealthReport {
    pub healthy: bool,
    pub timestamp_ms: u64,
    pub cas_objects: usize,
    pub agents: usize,
    pub kg_nodes: usize,
    pub kg_edges: usize,
    pub active_sessions: usize,
    pub embedding_backend: String,
    pub degradations: Vec<Degradation>,
    pub roundtrip_ok: bool,
    pub roundtrip_ms: u64,
}

pub struct Degradation {
    pub component: String,
    pub severity: String,
    pub message: String,
}
```

CLI 输出:
```
$ aicli health
System Health: HEALTHY (with degradations)
  CAS objects:    42
  Agents:         3
  KG nodes:       12
  KG edges:       7
  Active sessions: 1
  Embedding:      stub (BM25 only)
  Roundtrip:      OK (2ms)
  Degradations:
    ⚠ [embedding] Vector search unavailable, BM25 fallback active
    ⚠ [llm] Summarizer unavailable, context L0 returns heuristic
```

**估算**: ~100 行新代码, 4 个新测试
**验收**:
```
health → 显示 HEALTHY/DEGRADED/UNHEALTHY + 详细组件状态
health (EMBEDDING_BACKEND=stub) → 显示 embedding 降级
health (full) → 显示 HEALTHY, 无降级
```
**Soul 2.0**: 公理 10（会话一等公民）— 健康是会话上下文的一部分; 系统诚实报告自身状态

**AIOS 对标**: Zylos 2026 的层级健康端点模型。当前实现是 Level 1 (结构化自诊)。Level 2 (自动修复) 是 Node 14+ 方向。

---

#### F-8: Signal Repairs Sprint（信号修复冲刺）

四个快速修复，每个不超过 10 行代码，但对整体通过率影响显著。

**F-8a: Events History Filter (B14)**

```rust
// src/bin/aicli/commands/handlers/events.rs:77-89
// 修复: history 分支使用顶层 agent_id 变量而非单独提取 --agent-filter
Some("history") => {
    let since_seq = extract_arg(args, "--since")
        .and_then(|s| s.parse().ok());
    let limit = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok());
    let req = plico::api::semantic::ApiRequest::EventHistory {
        since_seq,
        agent_id_filter: agent_id.clone(),  // ← 使用顶层变量
        limit,
    };
    kernel.handle_api_request(req)
}
```

验收: `events history --agent tester` → 只返回 tester 的事件

**F-8b: Edge Display (B27)**

```rust
// 修复 edge 创建输出中的空节点名
// 在 print_result 中 edge 创建的 message 应包含节点名
// 或在创建 edge 的 handler 中构造完整的反馈消息
```

验收: `edge --from A --to B` → "Edge created: A --[RelatedTo]--> B"

**F-8c: Context Direct Load Degradation (B31)**

```rust
// src/fs/context_loader.rs — context_load 直接路径:
// 当请求 L0 但内容长度超过阈值时，应执行降级并标记
// 目前只有 assemble 路径会降级
```

验收: `context --cid <long_text> --layer L0` → 返回压缩内容, 标记 "[L0, degraded from L2]"

**F-8d: MCP Recall Test (B34)**

```rust
// 修复 dispatch_plico_recall_semantic_works 测试
// 可能是 MCP dispatch 路径的 recall 语义映射断裂
```

验收: `cargo test dispatch_plico_recall_semantic_works` → 通过

**估算**: ~40 行总修改, 4 个新测试
**Soul 2.0**: 公理 1（token 稀缺）— 事件过滤节省 token; 公理 6（结构先于语言）— 每个输出必须结构完整

---

## 5. 代码影响估算

| 特性 | 新代码 | 修改代码 | 新测试 | 主要文件 |
|------|--------|---------|--------|---------|
| F-1 Tier Match | 0 | ~5 行 | 3 | `handlers/memory.rs` |
| F-2 Tool Limit | 0 | ~15 行 | 3 | `builtin_tools.rs` |
| F-3 Permission CLI | ~80 行 | ~5 行 | 5 | `handlers/permission.rs` (新) + `mod.rs` |
| F-4 Delegate Name | 0 | ~10 行 | 2 | `handlers/agent.rs` 或 `handlers/task.rs` |
| F-5 Memory Linking | ~60 行 | ~10 行 | 4 | `kernel/ops/memory.rs` |
| F-6 Consolidation | ~80 行 | ~20 行 | 5 | `kernel/ops/session.rs` + `api/semantic.rs` |
| F-7 Health Report | ~100 行 | ~10 行 | 4 | `kernel/ops/dashboard.rs` + `handlers/` |
| F-8 Signal Repairs | 0 | ~40 行 | 4 | `handlers/events.rs` + `context_loader.rs` + graph |
| **总计** | **~320 行** | **~115 行** | **30** | |

**总估算**: ~435 行代码变更, 30 个新测试

对比: Node 12 设计 1200 行变更, 实际完成 50%。Node 13 有意控制范围，确保完成率 > 80%。

---

## 6. 实施计划

### 第一周: 快速胜利（F-1, F-2, F-8）

```
Day 1: F-1 (Tier Match) — 5 行修改, 立即可验证
       F-8a (Events Filter) — 1 行修改
Day 2: F-2 (Tool Limit) — 15 行 + dedup
       F-8b (Edge Display) — 10 行
Day 3: F-8c (Context Direct Load) — 15 行
       F-8d (MCP Recall Test) — 调查 + 修复
Day 4: 全部 dogfood 验证 + 测试补齐
```

**第一周预期**: 6 个 bug 修复, Node 7-8 得分 2/3→3/3, Node 11 得分 2/4→3/4

### 第二周: 权限 + 委托（F-3, F-4）

```
Day 1-2: F-3 (Permission CLI) — 新 handler + routing
Day 3:   F-4 (Delegation Name) — 10 行修改
Day 4:   集成测试 + dogfood 验证
         验证: permission grant → send → delegate 全链路
```

**第二周预期**: B29 修复 → B24 连锁修复, Node 1-2 得分 8/11→9/11

### 第三周: 记忆进化（F-5, F-6）

```
Day 1-2: F-5 (Memory Linking) — 60 行 + KG 测试
Day 3-4: F-6 (Consolidation Feedback) — 80 行 + session 测试
         验证: remember → explore 有边; session-end → consolidation 报告
```

**第三周预期**: A-4/A-5 从 ❌ → ✅, Node 12 得分 5/10→8/10

### 第四周: 系统健全（F-7）

```
Day 1-2: F-7 (Health Report) — 100 行 + CLI/MCP 双入口
Day 3:   全量 dogfood 回归测试 (54+ 测试项)
Day 4:   文档更新 + 下一个审计报告
```

**第四周预期**: Node 9 得分 1/3→2/3, 整体通过率 66.7%→83%+

---

## 7. 验收评分卡

### 预期得分提升

| Node | 描述 | 当前 | F-1 | F-2 | F-3 | F-4 | F-5 | F-6 | F-7 | F-8 | 预期 |
|------|------|------|-----|-----|-----|-----|-----|-----|-----|-----|------|
| 1–2 | 基础能力 | 8/11 (73%) | +B28 | | +B29 | | | | | | 10/11 (91%) |
| 3 | Tenant | 5/5 (100%) | | | | | | | | | 5/5 (100%) |
| 4 | 协作 | 6/7 (86%) | | | | +B30 | | | | | 7/7 (100%) |
| 5 | MCP | 3/4 (75%) | | | | | | | | +B34 | 4/4 (100%) |
| 6 | Budget | 3/4 (75%) | | | | +B30 | | | | | 4/4 (100%) |
| 7–8 | 工具 | 1/3 (33%) | | +B26 | +B29 | | | | | | 3/3 (100%) |
| 9 | 韧性 | 1/3 (33%) | | | | | | | +健康 | | 2/3 (67%) |
| 10 | 正名 | 2/3 (67%) | | | | | | | | +B14 | 3/3 (100%) |
| 11 | 落地 | 2/4 (50%) | | | | | | | | +B14 | 3/4 (75%) |
| 12 | 觉知 | 5/10 (50%) | | | | | +A-4 | +A-5 | | +B14,B31 | 9/10 (90%) |
| **总计** | | **36/54 (66.7%)** | | | | | | | | | **50/54 (92.6%)** |

### 每个特性的验收标准

| 特性 | 必须通过 | 验证命令 |
|------|---------|---------|
| F-1 | `--tier long-term` → recall 显示 [LongTerm] | `remember --tier long-term + recall` |
| F-2 | `limit:1` 返回 1 条, 无重复 | `tool call cas.search '{"query":"x","limit":1}'` |
| F-3 | permission grant/check/revoke 全路径可用 | `permission grant --agent A --action delete` |
| F-4 | `delegate --to <name>` 成功 | `delegate --from A --to B --desc "task"` |
| F-5 | remember 后 explore 有新边 | `remember + explore --scope full` |
| F-6 | session-end 输出 consolidation 报告 | `session-end` 输出含 "Consolidation:" |
| F-7 | `health` 返回结构化状态 | `health` → 含 degradations 列表 |
| F-8a | `events history --agent X` 只返回 X 的事件 | `events history --agent tester` |
| F-8b | edge 创建显示完整节点名 | `edge --from A --to B` |
| F-8c | `context --layer L0` 长文本降级 | `context --cid <long> --layer L0` |
| F-8d | MCP recall 测试通过 | `cargo test dispatch_plico_recall_semantic_works` |

---

## 8. Soul 2.0 公理对齐矩阵

| 公理 | Node 13 对应 | 对齐方式 |
|------|-------------|---------|
| 1. Token 最稀缺 | F-2 (limit), F-8a (filter), F-8c (L0 降级) | 参数执行 = token 节省 |
| 2. 意图先于操作 | F-4 (name delegation) | 说名字不说 UUID |
| 3. 记忆跨越边界 | F-1 (tier), F-5 (linking), F-6 (consolidation) | 层级忠实 + 关联持久化 |
| 4. 共享先于重复 | F-5 (linking across agents) | KG 边连接不同 agent 的记忆 |
| 5. 机制不是策略 | F-3 (permission), F-6 (consolidation threshold) | 权限是机制; 提升阈值是机制参数 |
| 6. 结构先于语言 | F-2 (JSON params), F-7 (structured health) | 参数和健康都是结构化数据 |
| 7. 主动先于被动 | F-7 (health degradation proactive) | 系统主动报告降级 |
| 8. 因果先于关联 | F-5 (linking with relevance scores) | 链接权重 = 可量化因果证据 |
| 9. 越用越好 | F-6 (access_count → tier promotion) | 多次访问 → 自动升级 |
| 10. 会话一等公民 | F-6 (session-end consolidation), F-7 (sessions in health) | 会话结束触发学习 |

**全部 10 条公理至少被 1 个特性覆盖。**

---

## 9. AIOS 路线图对齐

```
                  AIOS v0.3 (Rutgers)              Plico Node 13
                  ─────────────────              ────────────────
Memory Manager:   RAM + LRU-K eviction           4-tier + tier enforcement (F-1)
Storage Manager:  Disk persistence               CAS + persistent sessions (Node 12)
Agent Scheduler:  FIFO/RR/Priority              Agent lifecycle (already working)
Tool Manager:     Tool loading + resolution      37 tools + limit enforcement (F-2)
Access Manager:   Agent access control           Permission CLI + check (F-3)

                  AIP (Mar 2026)                  Plico Node 13
                  ─────────────                  ────────────────
Agent Identity:   Ed25519 + IBCTs               Name registry + resolve_agent (F-4)
Delegation Chain: Append-only token chain        Name-based delegation (F-4)
Scope Attenuation: Datalog policies             [Node 14+]
Provenance:       Completion blocks             [Node 14+]

                  Four-Layer (Apr 2026)           Plico Node 13
                  ─────────────────              ────────────────
Knowledge Layer:  Permanent, shared             CAS + shared scope
Memory Layer:     Per-agent, decay              4-tier + consolidation (F-1, F-6)
Wisdom Layer:     Revision-gated                [Node 14+: procedural → wisdom]
Intelligence:     Ephemeral, per-invocation     Ephemeral tier + session scope

                  Zylos Observability (Mar 2026)  Plico Node 13
                  ──────────────────────────      ────────────────
Health Endpoints: Cognitive readiness            Health report (F-7)
Self-Diagnostic:  Agent monitors itself          Degradation detection (F-7)
Fleet Health:     Buddy systems + metrics        [Node 14+]
Self-Healing:     Detect-diagnose-repair         [Node 14+]
```

---

## 10. Node 13 → Node 14 过渡预测

```
如果 Node 13 实现 > 80%（目标 92.6%），Node 14 的方向:

链式推导:
  F-1 (tier 忠实) → 衰减机制可以安全叠加 → NornicDB 式半衰期
  F-5 (memory linking) → 跨 agent 链接 → 联邦式知识图谱
  F-6 (consolidation) → LLM 辅助整合 → DreamCycle
  F-3 (permission) → capability tokens → AIP 协议
  F-7 (health) → 自动恢复 → self-healing patterns

Node 14 候选主题:
  A) "化" (Metamorphosis) — 系统开始自我进化
     - 记忆衰减 + 遗忘
     - LLM 辅助 DreamCycle（在接口层，非内核）
     - AIP 可验证委托
     - Self-healing auto-repair

  B) "联" (Federation) — 多实例协作
     - 跨 Plico 实例的 KG 联邦
     - Agent Card 发布和发现
     - ACP 式协商生命周期
     - 分布式 session

这两个方向取决于 Node 13 的实际完成度和 dogfood 反馈。
```

---

## 11. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-5 linking 在 stub 模式下无 semantic 信号 | 高 | 仅 tag 匹配有链接 | BM25 + tag 双路径; 真实 embedding 环境另行验证 |
| F-6 consolidation access_count 未在 memory 结构中持久化 | 中 | 跨进程 access_count 归零 | 在 MemoryEntry 持久化时包含 access_count |
| F-3 permission 内核 API 签名与设计不匹配 | 低 | 需要适配 | 先阅读 permission.rs, 按实际 API 适配 handler |
| F-7 health 需要新增 ApiResponse 字段 | 低 | 编译变更 | 新增 `health_report` 字段, Option 类型不影响已有代码 |
| Node 12 的 50% 未完成部分拖累 Node 13 | 中 | 依赖 A-4/A-5 的代码路径 | F-5/F-6 重新实现而非修补 |

---

## 12. 度量标准

| 指标 | 当前基线 | Node 13 目标 | 测量方式 |
|------|---------|-------------|---------|
| 总通过率 | 36/54 (66.7%) | 50/54 (92.6%) | dogfood 全量测试 |
| P1 HIGH bug | 2 (B28, B29) | 0 | dogfood 验证 |
| P2 MEDIUM bug | 7 | 2 (B31 partial, B34) | dogfood 验证 |
| 新增测试 | 399 | 429 (+30) | cargo test |
| MCP 测试失败 | 1 | 0 | cargo test --package plico-mcp |
| 代码变更 | — | ~435 行 | git diff --stat |
| 单测通过 | 370 lib + 29 mcp | 400+ lib + 30 mcp | cargo test |

---

*Node 13 的核心信念: 一个 AIOS 的价值不在于它有多少功能，而在于它的每个功能是否忠实传导信号。*
*"穷则变，变则通，通则久。" — 觉知之后是通，通之后才能久。*

---

*文档基于全新 dogfood 实例 `/tmp/plico-n13-dogfood`，零历史数据污染。*
*所有 exit code 验证使用无管道方法，消除 `$?` 捕获误差。*
*AIOS 校准来源: AIP (arXiv:2603.24775), Four-Layer (arXiv:2604.11364), SleepGate (arXiv:2603.14517), Zylos Agent Observability (2026-03), Mem0 State of AI Memory (2026-04), AIOS v0.3 (Rutgers)。*
