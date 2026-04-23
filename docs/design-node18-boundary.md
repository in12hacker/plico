# Plico 第十八节点设计文档
# 界 — 界面保真与存储升级

**版本**: v1.0
**日期**: 2026-04-23
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: JSON-First 输出 + 严格输入解析 + KG 存储升级(redb) + 跨 Agent 共享记忆 + CLI Handler 测试 + warm_context 修复
**前置**: 节点 17 ✅（88%）— B49 修复, 效果合约, P0 测试覆盖(+32), V-06 修复, F-3/F-6 部分完成
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n18-audit`，干净环境）+ 全量源码逐文件 review + cargo test 1057 全通过 + KG 存储性能实测 + Soul 2.0 十条公理逐条验证
**信息来源**: `docs/dogfood-audit-n17.md` + redb 4.0 (Pure Rust ACID KV) + EverMemOS (arXiv:2601.02163, Jan 2026) / ContextOS (2026) / Structured Output Patterns (2026 industry) + NLAH (arXiv:2603.25723) / TDAD (arXiv:2603.08806) / Rust AI Infrastructure Shift (OSS Insight)

---

## 0. AI 第一人称推演：为什么是"界"

### 层次一：我默认和人类说话，不和机器说话

```
$ aicli agent --register --name "agent-a"
Agent ID: 6871aa4d-705e-430e-a9df-8649364f2258
```

这是我的默认输出——一行纯文本，`Agent ID:` 后面跟一个 UUID。想解析？写正则。

```
$ AICLI_OUTPUT=json aicli agent --register --name "agent-b"
{"ok":true,"version":"18.0.0","agent_id":"..."}
```

这是 opt-in 的 JSON 输出。但为什么是 opt-in？我是一个 **AI 操作系统**。我的主要消费者是 AI Agent，不是人类终端用户。

2026 年 1 月，Anthropic Claude 结构化输出 GA——所有 agent API 默认 JSON Schema 约束。2026 年全行业共识："The most common failure mode in production AI agents is not bad reasoning. It is good reasoning with bad output formatting."（Agentmelt, 2026）

Soul 2.0 公理 6 说："结构先于语言——ApiRequest/ApiResponse（JSON）是唯一的内核接口"。但我的 CLI 默认输出是人类文本，等于在内核接口之上加了一层**信息降级**。Agent 必须知道 `AICLI_OUTPUT=json` 这个环境变量才能获得结构化数据。这不是"机制不是策略"，这是"策略凌驾于机制"。

### 层次二：我的图脑每学一件事就重写整个笔记本

每次 KG 添加一条边，`PetgraphBackend::persist()` 执行：

```
1. 读取全部 nodes → serialize → 写临时文件 → rename（kg_nodes.json）
2. 读取全部 out_edges → 展平为 Vec<EdgeRecord> → serialize → 写临时文件 → rename（kg_edges.json）
```

Dogfood 环境中，424 条边 = 272KB 的 `kg_edges.json`。每添加 1 条边，重写 272KB。这是 O(n) per write。

类比：一个人每次记住一件新事，就把整本日记从头到尾重新抄一遍。

当前规模（~500 边）这只是浪费 I/O。但 Soul 2.0 公理 9（越用越好）意味着 KG 会持续增长。在 5,000 边时，每次 persist ~2.7MB；50,000 边时 ~27MB。到那时系统会变得无法响应。

redb 4.0（纯 Rust 嵌入式 KV）提供了完美的解决方案：
- **ACID 事务**：节点和边可以在同一事务中原子修改
- **增量写**：O(log n) per write, 只写变更的 B-tree 页
- **MVCC**：读不阻塞写
- **crash-safe**：copy-on-write B-tree，无需手动 WAL
- **零外部依赖**：纯 Rust，编译时链接

### 层次三：我的共享记忆可见但不可触

```bash
# Agent A 存共享记忆
$ remember "cross-agent knowledge" --scope shared --agent $AG_A
Memory stored for agent 'A'

# Agent B 尝试检索
$ recall --scope shared --agent $AG_B
No memories.
```

`MemoryScope::Shared` 的机制存在——Agent A 的记忆被标记为 shared。但 Agent B 检索时，`recall` 只查询 Agent B 自己的记忆空间。**跨 Agent 共享检索的路径不存在。**

这违反了 Soul 2.0 公理 4："共享先于重复"。Agent 之间能发现彼此（`discover`），但不能访问彼此的共享知识。这就像两个同事坐在同一间办公室，能看到对方，但不能共享文件。

EverMemOS (arXiv:2601.02163, Jan 2026) 的 Reconstructive Recollection 展示了跨会话记忆如何被"按需重组"给调用者。ContextOS 的 Memory Tiering 区分了 "user-scoped vs agent-scoped memory"。Plico 有 scope 机制但缺 retrieval 路径。

### 层次四：三个静默降级 Bug 形成模式

| Bug | 输入 | 预期 | 实际 | 模式 |
|-----|------|------|------|------|
| B49 (N17 已修) | `put "content"` | 存入 "content" | 存入 "" | 位置参数忽略 |
| B50 (未修) | `edge --type caused_by` | Causes 类型 | RelatedTo | 无效枚举静默降级 |
| B52 (新发现) | `update --cid X "new"` | 更新为 "new" | 更新为 "" | 位置参数忽略 |

共同根因：**CLI 解析层在边界处丢失信息，且不报告丢失。**

B49 被 N17 修复了，但修复是定点的——只改了 `cmd_create`。`cmd_update` 有完全相同的代码模式（只用 `extract_arg(args, "--content")`）。B50 的 `parse_edge_type` 用 `_ => RelatedTo` 作为 catchall。

这三个 Bug 都是**界面保真问题**：数据通过 CLI 界面时发生了静默降级。在人类视角这可能只是"便利性"降级，但在 AI 视角这是**信息熵增加**——有序变成无序，因果变成关联，内容变成空白。

### 层次五：EverMemOS 的启示——记忆不只是存储

EverMemOS 引入了三个 Plico 目前缺失的概念：

1. **MemCell**（原子记忆单元）：包含事件轨迹 + 事实三元组 + **Foresight 信号**（时间约束的预测/计划）。Plico 的 `MemoryEntry` 有 content + tags + tier，但没有 Foresight。

2. **MemScene**（主题场景）：MemCell 的语义聚类，形成更高层结构。Plico 的 tier consolidation（L0→L1→L2）做了时间维度的合并，但缺少**语义维度的聚类**。

3. **Reconstructive Recollection**：不是简单检索，而是**按需重组**——根据当前问题选择必要且充分的记忆片段。ContextOS 的 "Context-Dependent Gravity" 是同一概念。Plico 的 `context assemble` 有 budget 控制，但没有"按当前意图加权"的能力。

这些是 N19+ 的方向，但 Node 18 需要为它们奠基——特别是修复跨 Agent 共享检索（公理4的基础），和完善会话上下文（B51 修复）。

### 推演结论

Node 15 修复了**输入安全**。Node 16 修复了**持久化和幽灵防御**。Node 17 修复了**效果合约和 P0 测试**。

Node 18 的使命：**修复系统所有界面处的信息降级，升级不可持续的存储模式，消除遗留技术债。**

"界"的三层含义：
1. **界面保真**：CLI 输出默认 JSON，解析不丢失信息
2. **边界防御**：无效输入在边界处被拒绝，不静默降级
3. **层界升级**：KG 存储从 JSON 全量写升级到 redb 增量写

---

## 1. 审计发现总结

### 1.1 N17 达成率

| 维度 | 承诺 | 实现 | 验证 |
|------|------|------|------|
| D1 输入保真 | B49 修复 | ✅ 100% | `put "content"` → 正确 CID |
| D2 效果合约 | 前后置条件 | ✅ 100% | `semantic_create` 空内容拒绝 + CID 可检索断言 |
| D3 工具合约 | P0 工具 | 🟡 70% | 内核层覆盖，dispatch 层未独立添加 |
| D4 P0 测试 | ≥24 tests | ✅ 100% | 32 tests (builtin_tools 16 + persistence 8 + execution 8) |
| D5 V-06 修复 | auto_summarize 可选 | ✅ 100% | `PLICO_AUTO_SUMMARIZE=1` 才启用 |
| D6 CLI 审计 | 16 handlers | 🟡 60% | 仅 crud.rs 审计完成 |

### 1.2 新发现 Bug + 技术债

| ID | 严重度 | 描述 | 根因 | 发现方式 |
|----|--------|------|------|---------|
| **B50** | P2 | `edge --type caused_by` → 存储为 RelatedTo | `parse_edge_type` catchall `_ => RelatedTo` | Dogfood 实测 |
| **B51** | P1 | `session-start` warm_context 返回 UUID 非 CAS CID | `prefetch.declare_intent()` 返回 assembly_id | Dogfood: `get <UUID>` → InvalidCid |
| **B52** | P0 | `update --cid X "new content"` 位置参数忽略 | `cmd_update` 只用 `extract_arg("--content")` | Dogfood: update → SHA256("") |
| **TD-1** | P1 | KG persist 全量 JSON 重写 | `backend.rs` persist() O(n) per write | 代码审查 |
| **TD-2** | P1 | 跨 Agent 共享记忆不可检索 | recall 只查询调用者自己的 scope | Dogfood: Agent B recall shared → "No memories" |
| **TD-3** | P0 | 默认输出人类格式，非 JSON | `print_result()` 默认非 JSON | 公理 6 审计 |
| **TD-4** | P2 | KG 80% Fact 节点孤立无边 | 早期 dogfood 未执行 edge linking | N17 报告 §10.1 |

### 1.3 单元测试覆盖率

| 指标 | 值 |
|------|-----|
| 总测试数 | **1057** (537 lib + 其余 integration/doc/harness) |
| 有效代码文件 | **95** |
| 有测试覆盖文件 | **60** |
| **文件覆盖率** | **63.2%** (60/95) |
| **P0 零测试文件** | **1** (`kernel/mod.rs` 1910 行，依赖 integration 间接覆盖) |
| CLI handler 无测试 | **5** (graph.rs, agent.rs, crud.rs, memory.rs, skills.rs) ~883 行 |

---

## 2. Node 18 六大维度

### D1: JSON-First 输出（公理 6 修复）

**问题**: CLI 默认人类格式，Agent 需知道 `AICLI_OUTPUT=json`。
**目标**: 反转默认值——默认 JSON，`AICLI_OUTPUT=human` 时人类格式。
**度量**: 无环境变量时 CLI 输出为合法 JSON。

### D2: 严格输入解析（B50 + B52 + CLI 审计完成）

**问题**: 3 个静默降级 Bug 形成模式——CLI 边界丢失信息。
**目标**: B50 修复（无效 edge type 返回错误）+ B52 修复（update 支持位置参数）+ 全部 handler 审计。
**度量**: 0 个 CLI 命令静默降级无效输入。

### D3: KG 存储升级（redb 增量 persist）

**问题**: KG persist 全量 JSON O(n)，272KB per write，不可扩展。
**目标**: 引入 redb 替换 KG JSON persist，支持增量写和 ACID 事务。
**度量**: KG write 延迟 < 1ms per edge（当前约 ~5ms for 272KB JSON）。

### D4: 跨 Agent 共享记忆（公理 4 修复）

**问题**: `MemoryScope::Shared` 存储有效但跨 Agent 检索不通。
**目标**: Agent B 可检索 Agent A 的 shared 记忆。
**度量**: `recall --scope shared --from <other_agent>` 返回跨 Agent 结果。

### D5: CLI Handler 测试覆盖

**问题**: 5 个 CLI handler 文件零测试（883 行）。
**目标**: 每个 handler 至少 4 个测试。
**度量**: ≥20 个新 CLI handler 测试。

### D6: Warm Context 修复（B51 + 公理 2/10）

**问题**: `session-start --intent` 返回不可检索的 UUID。
**目标**: warm_context 返回可通过 `get` 检索的 CAS CID。
**度量**: `get <warm_context>` 返回有效内容。

---

## 3. Node 18 特性清单

### F-1: JSON-First Output — 默认输出反转

**当前代码** (`commands/mod.rs:118`):
```rust
pub fn print_result(response: &ApiResponse) -> bool {
    if std::env::var("AICLI_OUTPUT").as_deref().ok() == Some("json") {
        println!("{}", serde_json::to_string_pretty(response).unwrap_or_default());
        return response.ok;
    }
    // ... 350+ lines of human formatting ...
}
```

**修复**:
```rust
pub fn print_result(response: &ApiResponse) -> bool {
    let format = std::env::var("AICLI_OUTPUT").unwrap_or_else(|_| "json".to_string());
    if format != "human" {
        println!("{}", serde_json::to_string_pretty(response).unwrap_or_default());
        return response.ok;
    }
    // ... human formatting (opt-in) ...
}
```

**向后兼容**: 现有脚本使用 `AICLI_OUTPUT=json` 不受影响。人类用户需设置 `AICLI_OUTPUT=human`（或 shell alias）。

**测试**:
- `test_default_output_is_json`: 无环境变量时输出可 JSON parse
- `test_human_output_opt_in`: `AICLI_OUTPUT=human` 时输出人类格式
- `test_json_output_contains_version`: JSON 包含 version 字段

### F-2: Strict Input Parsing — B50 + B52 修复

#### F-2a: B50 — edge type 无效值返回错误

```rust
// graph.rs — parse_edge_type 修复
pub fn parse_edge_type(s: &str) -> Result<KGEdgeType, String> {
    match s {
        "associates_with" => Ok(KGEdgeType::AssociatesWith),
        "follows" => Ok(KGEdgeType::Follows),
        "mentions" => Ok(KGEdgeType::Mentions),
        "causes" => Ok(KGEdgeType::Causes),
        "reminds" => Ok(KGEdgeType::Reminds),
        "part_of" => Ok(KGEdgeType::PartOf),
        "similar_to" => Ok(KGEdgeType::SimilarTo),
        "related_to" => Ok(KGEdgeType::RelatedTo),
        "has_participant" => Ok(KGEdgeType::HasParticipant),
        "has_artifact" => Ok(KGEdgeType::HasArtifact),
        "has_recording" => Ok(KGEdgeType::HasRecording),
        "has_resolution" => Ok(KGEdgeType::HasResolution),
        "has_fact" => Ok(KGEdgeType::HasFact),
        "supersedes" => Ok(KGEdgeType::Supersedes),
        _ => Err(format!(
            "Unknown edge type: '{}'. Valid: associates_with, follows, mentions, causes, \
             reminds, part_of, similar_to, related_to, has_participant, has_artifact, \
             has_recording, has_resolution, has_fact, supersedes",
            s
        )),
    }
}
```

**影响传播**: 所有调用 `parse_edge_type` 的地方需要处理 `Result`。

#### F-2b: B52 — cmd_update 支持位置参数 + 空内容检查

```rust
pub fn cmd_update(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    if cid.is_empty() {
        return ApiResponse::error("update requires --cid <CID>");
    }

    let content = extract_arg(args, "--content")
        .or_else(|| {
            // 位置参数: 在 --cid 之后的第一个非 flag 参数
            let cid_idx = args.iter().position(|a| a == "--cid")
                .map(|i| i + 2)  // skip --cid and its value
                .unwrap_or(2);   // or skip "update <cid>"
            args.get(cid_idx).cloned().filter(|a| !a.starts_with("--"))
        })
        .unwrap_or_default();

    if content.is_empty() {
        return ApiResponse::error("update requires content: update --cid <CID> --content <text>");
    }

    let new_tags = extract_tags_opt(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_update(&cid, content.into_bytes(), new_tags, &agent_id, "default") {
        Ok(new_cid) => ApiResponse::with_cid(new_cid),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
```

#### F-2c: CLI Handler 系统审计（完成 F-6 遗留）

| Handler | 文件 | 审计项 | 修复 |
|---------|------|--------|------|
| cmd_remember | memory.rs | 位置参数 content | 支持 `remember "text"` |
| cmd_send | messaging.rs | 位置参数 content | 审计确认 |
| cmd_intent | intent.rs | 位置参数 text | 审计确认 |
| cmd_add_node | graph.rs | --label 位置参数 | 审计确认 |
| 全部 | 全部 | agent resolve_agent | 审计确认 |

### F-3: KG 存储升级 — redb 增量 persist

**设计原则**: 最小侵入——替换 `PetgraphBackend` 的 persist/load，不改变内存中的 HashMap 结构。

**依赖变更** (Cargo.toml):
```toml
[dependencies]
redb = "4.0"
```

**存储 schema**:

```rust
use redb::{Database, TableDefinition, ReadableTable};

const KG_NODES: TableDefinition<&str, &[u8]> = TableDefinition::new("kg_nodes");
const KG_EDGES: TableDefinition<&str, &[u8]> = TableDefinition::new("kg_edges");
// Edge key format: "{src}|{dst}|{edge_type}" → serialized KGEdge
```

**persist 改造**:

```rust
impl PetgraphBackend {
    fn persist_node(&self, node_id: &str, node: &KGNode) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(KG_NODES).unwrap();
            let data = serde_json::to_vec(node).unwrap();
            table.insert(node_id, data.as_slice()).unwrap();
        }
        write_txn.commit().unwrap();
    }

    fn persist_edge(&self, src: &str, dst: &str, edge: &KGEdge) {
        let Some(ref db) = self.db else { return };
        let key = format!("{}|{}|{:?}", src, dst, edge.edge_type);
        let write_txn = db.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(KG_EDGES).unwrap();
            let data = serde_json::to_vec(edge).unwrap();
            table.insert(key.as_str(), data.as_slice()).unwrap();
        }
        write_txn.commit().unwrap();
    }

    fn remove_node_from_db(&self, node_id: &str) {
        let Some(ref db) = self.db else { return };
        let write_txn = db.begin_write().unwrap();
        {
            let mut table = write_txn.open_table(KG_NODES).unwrap();
            table.remove(node_id).ok();
        }
        write_txn.commit().unwrap();
    }
}
```

**load 改造**: 启动时从 redb 读取所有 nodes/edges 到内存 HashMap（与当前 JSON load 等价）。

**迁移路径**: 如果检测到旧的 `kg_nodes.json` 存在，一次性导入到 redb，然后删除 JSON 文件。

**性能对比**:

| 操作 | 当前 (JSON) | redb | 改善 |
|------|------------|------|------|
| 添加 1 条边 | ~5ms (272KB full write) | <0.1ms (single B-tree insert) | **50x** |
| 添加 1 个节点 | ~3ms (32KB full write) | <0.1ms | **30x** |
| 查询节点 | <0.01ms (HashMap) | <0.01ms (HashMap, same) | — |
| 启动加载 | ~2ms (JSON parse) | ~3ms (redb scan) | ~1.5x slower |
| crash recovery | ❌ (rename 原子性只保证单文件) | ✅ (ACID COW B-tree) | **关键改善** |

### F-4: 跨 Agent 共享记忆检索

**当前限制**: `kernel.recall()` 只查询指定 `agent_id` 的记忆。

**新增 API**:

```rust
// ApiRequest 新增
ApiRequest::RecallShared {
    caller_agent_id: String,
    target_agent_id: Option<String>,  // None = 所有 agent 的 shared
    query: Option<String>,
    limit: usize,
}
```

**内核实现**:

```rust
pub fn recall_shared(
    &self, caller_id: &str, target_id: Option<&str>,
    query: Option<&str>, limit: usize,
) -> Vec<MemoryEntry> {
    let agents = if let Some(target) = target_id {
        vec![target.to_string()]
    } else {
        self.scheduler.list_agents().into_iter()
            .filter(|a| a.id != caller_id)
            .map(|a| a.id)
            .collect()
    };

    let mut results = Vec::new();
    for agent_id in &agents {
        let entries = self.memory.recall(agent_id, "default", query);
        for entry in entries {
            if entry.scope == MemoryScope::Shared {
                results.push(entry);
            }
        }
    }

    results.sort_by(|a, b| b.importance.partial_cmp(&a.importance).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    results
}
```

**CLI 支持**:
```bash
recall --scope shared --from <agent_name_or_id>  # 指定 agent
recall --scope shared --all                       # 所有 agent 的 shared
```

### F-5: CLI Handler 单元测试

每个 handler 至少 4 个测试，覆盖：正常路径、边界输入、错误路径、参数解析。

| Handler | 文件 | 行数 | 新测试 |
|---------|------|------|--------|
| cmd_add_node/edge/etc | graph.rs | 245 | 5 |
| cmd_agent/agents | agent.rs | 190 | 4 |
| cmd_create/read/update/delete | crud.rs | 182 | 5 |
| cmd_remember/recall | memory.rs | 137 | 4 |
| cmd_skills_* | skills.rs | 129 | 4 |
| **合计** | | **883** | **22** |

测试方式：使用 `crate::kernel::tests::make_kernel()` 直接调用 handler 函数（不启动 CLI 进程）。

### F-6: Warm Context 修复（B51）

**当前代码** (session.rs):
```rust
let warm_context: Option<String> = if let Some(ref hint) = intent_hint {
    let budget = 4096;
    let assembly_id = prefetch.declare_intent(agent_id, hint, ...);
    Some(assembly_id)  // UUID, not CAS CID!
} else {
    None
};
```

**修复**：将 assembly 结果存入 CAS，返回 CAS CID。

```rust
let warm_context: Option<String> = if let Some(ref hint) = intent_hint {
    let budget = 4096;
    let assembly_id = prefetch.declare_intent(agent_id, hint, budget);
    // 尝试获取 assembled context 并存入 CAS
    match prefetch.fetch_assembled_context(&assembly_id) {
        Some(assembled) => {
            let content = serde_json::to_vec(&assembled).unwrap_or_default();
            if !content.is_empty() {
                match self.fs.create(content, vec!["warm-context".into()], agent_id.to_string(), Some(hint.clone())) {
                    Ok(cid) => Some(cid),
                    Err(_) => Some(assembly_id),  // fallback to assembly_id
                }
            } else {
                Some(assembly_id)
            }
        }
        None => Some(assembly_id),  // prefetch not ready yet
    }
} else {
    None
};
```

---

## 4. 前沿研究对标

### 4.1 redb 4.0 — KG 存储升级基础

| redb 特性 | Plico 应用 | Node 18 目标 |
|-----------|-----------|-------------|
| ACID 事务 | 节点+边原子修改 | ✅ F-3 |
| 增量写 O(log n) | 替代 JSON 全量写 | ✅ F-3 |
| MVCC | 读不阻塞写 | ✅ F-3 |
| Crash-safe COW | 替代 rename 原子性 | ✅ F-3 |
| Savepoints | 事务回滚 | N19+ |

### 4.2 EverMemOS — 记忆升级方向

| EverMemOS 概念 | Plico 现状 | Node 18 贡献 |
|---------------|-----------|-------------|
| MemCell (原子记忆) | MemoryEntry | — (已有) |
| MemScene (语义聚类) | ❌ | — (N19+) |
| Reconstructive Recollection | ❌ → ⚠️ | F-4 跨 Agent 检索是基础 |
| Foresight (时间约束预测) | ❌ | — (N20+) |
| 93% LoCoMo | — | 基准对标 |

### 4.3 ContextOS — 认知层参考

| ContextOS 特性 | Plico 现状 | Node 18 贡献 |
|---------------|-----------|-------------|
| Semantic Intent Router | ✅ IntentRouter | — |
| Context-Dependent Gravity | ❌ | — (N19+) |
| Active Forgetting | ⚠️ tier eviction | — |
| Context Budget | ✅ context_budget.rs | — |
| Tool Output Caching | ❌ | — (N19+) |

### 4.4 Structured Output 2026 行业标准

| 标准 | Plico 现状 | Node 18 目标 |
|------|-----------|-------------|
| JSON-first API | ❌ (human default) | ✅ F-1 |
| Schema enforcement | ✅ (tool schemas) | — |
| Reasoning envelope | ❌ | — (N19+) |

### 4.5 AIOS 2026 方向校准

```
Node 1-3:   ████████████████  CAS + 语义 FS + 搜索 = AIOS 存储层
Node 4-6:   ████████████████  Agent + 权限 + 消息 = AIOS 调度层
Node 7-8:   ████████████████  事件 + Delta = AIOS 驾具层
Node 9-10:  ████████████████  弹性 + 整流 = AIOS 韧性层
Node 11-12: ████████████████  4 层记忆 + 合并 = AIOS 记忆层
Node 13:    ████████████████  API v18 + MCP + Intent = AIOS 传导层
Node 14:    ████████████████  融合 = AIOS 集成层
Node 15:    ████████████████  输入安全 = AIOS 防御层(输入)
Node 16:    ██████████████░░  持久化 + 幽灵防御 = AIOS 持续层
Node 17:    ██████████████░░  效果合约 + P0 测试 = AIOS 诚信层
Node 18:    ░░░░░░░░░░░░░░░░  JSON-First + redb + 共享记忆 = AIOS 界面层  ← 当前
Node 19+:   ................  意图缓存 + 预热 + 认知层 = AIOS 主动层
```

---

## 5. 影响分析

### 5.1 F-1 (JSON-First) 影响

```
print_result() 默认分支变更:
  现有: AICLI_OUTPUT != "json" → 人类格式
  新增: AICLI_OUTPUT == "human" → 人类格式, 否则 JSON

影响范围:
  - 所有现有 shell 脚本使用 AICLI_OUTPUT=json → 不受影响
  - 人类用户直接调用 CLI → 需设 AICLI_OUTPUT=human 或 alias
  - Agent 调用 CLI → 改善（默认获得结构化输出）
  - plico-bootstrap.sh → 审计是否解析人类格式输出
```

### 5.2 F-3 (redb) 影响

```
新增依赖: redb = "4.0"
编译时间影响: +~10s (redb 是纯 Rust)

修改文件:
  - Cargo.toml: +1 dependency
  - fs/graph/backend.rs: persist/load 改造 (~100 行变更)
  - 新增: fs/graph/redb_backend.rs 或内联到 backend.rs

迁移路径:
  1. 检测 kg_nodes.json 存在 → 一次性导入 redb
  2. 导入成功后删除 JSON 文件
  3. 后续所有 persist 走 redb

回退方案:
  - 保留 JSON persist 为 feature flag: --features json-kg
  - 默认使用 redb
```

### 5.3 变更影响矩阵

| 特性 | 修改文件 | 新增行 | 修改行 | 新测试 | 新依赖 |
|------|---------|-------|--------|-------|--------|
| F-1 | commands/mod.rs, README, CLAUDE.md | ~5 | ~10 | 3 | 0 |
| F-2a | handlers/graph.rs | ~5 | ~15 | 3 | 0 |
| F-2b | handlers/crud.rs | ~10 | ~5 | 3 | 0 |
| F-2c | handlers/memory.rs, intent.rs | ~10 | ~5 | 2 | 0 |
| F-3 | Cargo.toml, graph/backend.rs | ~150 | ~50 | 8 | redb 4.0 |
| F-4 | kernel/ops/memory.rs, api/semantic.rs, handlers/memory.rs | ~80 | ~20 | 6 | 0 |
| F-5 | handlers/*.rs (5 files) | ~180 | 0 | 22 | 0 |
| F-6 | kernel/ops/session.rs | ~20 | ~10 | 3 | 0 |
| **合计** | | **~460** | **~115** | **50** | **1** |

---

## 6. 量化目标

| 指标 | N17 现状 | N18 目标 | 计算方式 |
|------|---------|---------|---------|
| 总测试数 | 1057 | **1107+** | +50 new |
| 文件覆盖率 | 63.2% (60/95) | **68%+** (65/95) | +5 handler files |
| 遗留 Bug | 3 (B50/B51/B52) | **0** | 全部修复 |
| 技术债 | 4 (TD-1~TD-4) | **1** (TD-4 KG 孤立节点 P2) | TD-1~3 修复 |
| 灵魂对齐（加权） | ~68% | **≥73%** | 公理 4 (+10) + 公理 6 (+15) |
| KG persist 延迟 | ~5ms (JSON) | **<0.1ms** (redb) | 50x 改善 |
| 新增依赖 | 0 | **1** (redb) | |

---

## 7. 实施计划

### Phase 1: JSON-First + B50/B52 修复（~1 day）

1. F-1: `print_result()` 默认反转
2. F-2a: `parse_edge_type` 返回 Result + 错误提示
3. F-2b: `cmd_update` 支持位置参数 + 空内容检查
4. F-2c: 审计 `cmd_remember`, `cmd_send`, `cmd_intent`, `cmd_add_node`
5. 测试: 11 个新测试

### Phase 2: CLI Handler 测试（~1 day）

1. F-5: graph.rs 5 tests
2. F-5: agent.rs 4 tests
3. F-5: crud.rs 5 tests
4. F-5: memory.rs 4 tests
5. F-5: skills.rs 4 tests

### Phase 3: KG redb 升级（~2 days）

1. 添加 `redb = "4.0"` 依赖
2. 实现 redb persist_node/persist_edge/remove
3. 实现 redb load_all 替代 JSON load
4. 迁移逻辑：JSON → redb 一次性导入
5. 测试: 8 个新测试
6. 性能验证: 1000 边写入基准测试

### Phase 4: 跨 Agent 共享记忆 + B51（~1.5 days）

1. F-4: `RecallShared` API + 内核实现
2. F-4: CLI `recall --scope shared --from <agent>` 支持
3. F-6: B51 warm_context 修复——存入 CAS 返回 CID
4. 测试: 9 个新测试

### Phase 5: Dogfood 验证（~0.5 day）

1. 干净环境验证全部修复
2. 验证 JSON 默认输出
3. 验证 redb KG persist
4. 验证跨 Agent 共享记忆
5. 回归测试 1057 + 50 新测试

---

## 8. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-1 JSON 默认可能破坏现有 shell 脚本 | 高 | 中 | plico-bootstrap.sh 审计 + AICLI_OUTPUT=human 回退 |
| F-3 redb 依赖增加编译时间 | 低 | 低 | redb 纯 Rust，无系统依赖 |
| F-3 redb 迁移可能丢失数据 | 低 | 高 | 迁移前备份 JSON，迁移后验证，失败回退 |
| F-4 跨 Agent 检索可能暴露隐私 | 中 | 高 | 仅 Shared scope 可跨 Agent，Private 永不暴露 |
| F-2a parse_edge_type Result 传播范围大 | 中 | 中 | 所有调用点逐一审计 |

---

## 9. 从 Node 18 到 Node 19 的推演

Node 18 完成后，Plico 将具备：
- JSON-First 输出（公理 6 修复）
- 严格输入解析（0 静默降级 Bug）
- redb 增量 KG persist（ACID + O(log n)）
- 跨 Agent 共享记忆检索（公理 4 修复）
- CLI handler 68%+ 测试覆盖
- warm_context 可检索（公理 2/10 修复）

**Node 19 展望**: **觉 (Awareness) — 主动性与越用越好**

基于 Node 18 的界面保真和存储基础，Node 19 将攻坚 Soul 2.0 最大差距——公理 7（主动先于被动）和公理 9（越用越好）：

1. **意图缓存 (Intent Cache)**: 相似意图命中历史结果，越用越快（公理 9 核心）
2. **上下文主动预热 (Context Pre-assembly)**: `session-start --intent` 后后台异步组装，Agent 下次查询零等待（公理 7 核心）
3. **Agent 工作模式学习 (Agent Profile)**: 记录 Agent 偏好的工具、常用标签、活跃时段，形成行为画像（公理 9 + EverMemOS Foresight 概念）
4. **Context-Dependent Gravity**: 基于当前意图重新加权检索结果（ContextOS 启发）

这将使 Plico 的 Soul 2.0 对齐度从 ~73% 提升到 ~80%+，跨越从"被动 OS"到"主动 OS"的临界点。

---

*文档基于 1057 个自动化测试 + 独立 Dogfood 实测 + 95 个源文件逐行审计 + 3 项网络研究(redb/EverMemOS/ContextOS/StructuredOutput) + Soul 2.0 十条公理逐条验证。*
*B50/B51/B52 通过真实 CLI 执行确认。TD-1~TD-4 通过代码审查和数据分析确认。*
*redb 4.0 / EverMemOS (arXiv:2601.02163) / ContextOS / Structured Output Patterns 均为 2026 年最新研究与实践。*
