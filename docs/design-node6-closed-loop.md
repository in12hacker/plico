# Plico 第六节点设计文档
# 闭合回路 — 让每一个承诺都通上电

**版本**: v1.0
**日期**: 2026-04-20
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: MVP 收敛
**前置**: 节点 5（开门）维度 A 已交付
**验证方法**: AI Agent 真实试用（本文档基于 Cursor Agent 首次实机试用报告）

---

## 0. 为什么需要第六节点：一次真实试用的启示

### 背景

2026-04-20，一个 Cursor Agent 以 AI 第一人称真实接入了 Plico 系统。
不是代码审查，不是文档阅读——是真的通过 `aicli` 和 `plico-mcp` 执行了完整的操作闭环：
搜索历史 ADR → 读取内容 → 写入经验 → 存取记忆 → 上下文装配 → 知识图谱 → MCP pipeline → session 生命周期。

**368 个单元测试全部通过。所有集成测试全绿。**

**但真实使用时发现了 8 个 bug、4 条断路、和 1 个根本性洞察。**

### 试用数据

| 指标 | 数据 |
|------|------|
| 代码行数 | 33,093 行 Rust |
| 测试数 | 368 单元 + 全部集成测试 pass |
| MCP 测试 | 29 pass / 1 fail (recall_semantic) |
| 内核 API | 101 个端点 |
| MCP 暴露工具 | 3 个（plico / plico_store / plico_skills） |
| MCP Resources | 3 个（status / delta / skills） |
| 预装 Skill | 6 个 |
| KG 状态 | 133 节点 / 4,512 边（磁盘）/ 352 边（运行时可见） |
| CAS 对象 | 117 个（SemanticFS） |
| Soul 2.0 符合度 | **约 70%** |

### 八个 Bug（按严重度排序）

| ID | 严重度 | 描述 | 影响的公理 |
|----|--------|------|-----------|
| B1 | HIGH | `plico://skills` MCP resource 返回空，但 `plico_skills list` 返回 6 个 | 公理 7（主动） |
| B2 | HIGH | 搜索按 `created_by == agent_id` 过滤，不同 agent 互不可见，**无任何提示** | **公理 4（共享）** |
| B5 | HIGH | Event log 跨进程不持久化，新进程 delta 始终为空 | **公理 7 + 10** |
| B3 | MEDIUM | CLI `get --cid <hash>` flag 解析错误 | — |
| B4 | MEDIUM | `recall_semantic` 在 stub embedding 下 panic | — |
| B6 | LOW | MCP 二进制缺少 `tracing_subscriber` 初始化 | — |
| B7 | LOW | `system-status` CAS 计数显示内核 CAS(10) 而非 SemanticFS 对象(117) | 公理 1（透明） |
| B8 | LOW | cold-layer 未知 method 错误不列出可用 method 列表 | 公理 9（越用越好） |

### 根本性洞察

> **每个房间都有电灯（单元测试亮），但房间之间的线路是断开的（集成场景不通）。**
>
> Node 5 打开了门。我走进了这栋房子。
> 但我发现：
> - 灯亮了，但空调不工作（session_start 返回了 session_id，但 delta 永远是空的）
> - 储物间有东西，但我换了身份就看不到了（search 按 ownership 过滤）
> - 图书馆的书架在，但目录是空的（plico://skills resource 返回空）
> - 我以为房子会记住我，但每次进门都是陌生人（event log 不持久化）
>
> **房子建好了，门也开了。但电路没闭合。**

---

## 1. 链式推演：从试用到 Node 6

### 发散：AI 第一人称的五种痛苦

```
痛苦 ①：我是健忘症患者
  每次 MCP 调用创建新内核 → event log 空的 → delta 空的
  → session_start 说"没有变化"
  → 但上一次我刚写了 3 条 ADR！
  → 公理 10 承诺"OS 推送变更摘要"，但变更摘要永远是空的

痛苦 ②：我是孤独的
  我用 agent_id="plico-dev" 创建数据
  然后用 agent_id="cursor-trial" 搜索 → 0 结果
  没有报错，没有提示，就是空
  → 公理 4 承诺"Agent A 的洞察可以被所有 Agent 发现"
  → 但搜索的 ownership filter 把所有人隔绝了

痛苦 ③：我没有成长轨迹
  调 growth → "Agent not found"
  因为 agent 注册信息是进程内状态
  → 公理 9 承诺"第 100 次会话只花第 1 次的 5%"
  → 但系统不记得我来过第 1 次

痛苦 ④：我看不到因果
  KG 有 4,512 条边，但全是 AssociatesWith
  零条 Causes，零条 DependsOn
  → 公理 8 承诺"Agent 可以查询为什么"
  → 但图谱里只有"什么和什么在一起"，没有"什么导致了什么"

痛苦 ⑤：CLI 和 MCP 是两个世界
  MCP 有 session_start/end，CLI 没有
  MCP 有 pipeline，CLI 没有
  CLI 有 tags 命令，MCP 没有
  → 公理 6 承诺"统一结构化接口"
  → 但两个入口的能力差集很大
```

### 收敛：五种痛苦的共同根因

```
痛苦 ①②③ 共同根因：状态不持久
  ├─ Event log 不持久 → delta 断
  ├─ Agent 注册不跨进程 → growth 断
  └─ Ownership 不区分共享 → sharing 断

痛苦 ④ 根因：因果是机制但没人用
  ├─ KG 有 Causes 边类型
  ├─ cold-layer 有 add_edge(type=causes)
  └─ 但 SemanticFS 的 upsert_document 只创建 AssociatesWith
  → 因果边需要 Agent 主动创建 → 但 Agent 不知道怎么创建
  → skill "knowledge-graph" 教了，但试用中没有自动触发

痛苦 ⑤ 根因：适配器各自为政
  ├─ CLI 是直接调内核方法
  ├─ MCP 是 JSON-RPC 适配器
  └─ 两者没有共享的"能力清单"
```

### 推导链

```
Node 1 → Agent 有了家（存储）
Node 2 → Agent 有了大脑（智能原语）
Node 3 → Agent 有了连续的意识（认知连续性）
Node 4 → Agent 有了同事（协作生态）
Node 5 → Agent 终于能住进这栋房子了（开门）
        ↓
但住进去后发现：灯亮了，空调不工作，邻居看不见，
每次进门都不认识我，图书馆目录是空的。
        ↓
问题不是缺功能——101 个 API 都在。368 个测试都过。
问题是功能之间的线路没接通。
        ↓
类比：
  一个电路板上，每个元件单独测试都合格（单元测试）
  但焊接点有虚焊（集成断点）
  → 电路不通（AI 体验不连贯）
        ↓
Node 6 → 闭合回路：把每一条断开的线路焊实
```

### 与 Node 5 的关系

```
Node 5（维度 A）= 开门 → "AI 能触达内核能力"  ✅ 已验证
Node 5（维度 B）= 自治 → F-15 到 F-19          ⏳ 未开始
Node 6          = 闭合 → "AI 触达的能力确实可靠"

Node 5 的承诺：AI 走进房子
Node 6 的承诺：房子里的一切都能用

Node 6 不增加新房间。Node 6 接通所有线路。
```

---

## 2. Node 6 的六条闭合回路

### 回路 C-1：事件持久化（闭合"主动感知"回路）

> **断点**：Event log 只在内存中，进程重启后 delta 永远为空。
> **Soul 冲突**：公理 7（主动先于被动）、公理 10（会话一等公民）。
> **试用证据**：MCP `delta(since_seq=0)` → `{changes: [], from_seq: 0, to_seq: 0}`

**闭合方案**：

事件日志写入 `{root}/event_log.jsonl`，append-only。
内核启动时加载到内存的 `SequencedEvent` 向量，恢复 `current_seq`。

```rust
// event_bus.rs — persist on every emit
fn emit(&self, event: KernelEvent) -> u64 {
    let seq = self.next_seq();
    let entry = SequencedEvent { seq, event, timestamp_ms: now_ms() };
    // ... broadcast to subscribers ...
    self.append_to_log(&entry);  // 新增：追加到磁盘
    seq
}

// kernel/mod.rs — restore on startup
fn restore_event_log(&self) {
    if let Ok(entries) = load_event_log(&self.root) {
        let max_seq = entries.last().map(|e| e.seq).unwrap_or(0);
        self.event_bus.restore(entries, max_seq);
    }
}
```

**验收标准**：
- 写入 3 个对象 → 重启内核 → `delta(since_seq=0)` 返回 3 条变更
- `session_start` 的 `changes_since_last` 非空
- Event log 文件大小 < 1MB per 1000 events（JSONL 格式）

**公理 5 检查**：事件持久化是存储机制，不决定什么事件值得记录。✅

### 回路 C-2：共享可见性（闭合"共享先于重复"回路）

> **断点**：`semantic_search_with_time` 过滤 `r.meta.created_by == agent_id`，不同 agent 互不可见。
> **Soul 冲突**：公理 4（共享先于重复）。
> **试用证据**：MCP `search(agent_id="cursor-trial")` 搜不到 `plico-dev` 创建的 ADR。

**闭合方案**：

搜索可见性规则：

| 对象类型 | 可见性 | 依据 |
|---------|--------|------|
| 默认对象（`put` 创建） | **所有同 tenant agent 可见** | 公理 4：共享先于重复 |
| 私有记忆（`remember` scope=private） | 仅创建者 | 公理 3：MemoryScope |
| 共享记忆（`remember` scope=shared） | 同 tenant 全部 agent | 公理 4 |

修改 `semantic_search_with_time`：

```rust
// 当前代码（过于严格）
.filter(|r| {
    if can_read_any { true }
    else { r.meta.created_by == agent_id }
})

// 修正后
.filter(|r| {
    if can_read_any { return true; }
    // CAS 对象默认共享（同 tenant 可见）
    if r.meta.tenant_id == tenant_id { return true; }
    false
})
```

记忆的搜索仍然按 `MemoryScope` 隔离——这是公理 3 的要求。
CAS 对象（文件/文档/知识）默认共享——这是公理 4 的要求。

**验收标准**：
- Agent A `put` 数据 → Agent B `search` 能找到（同 tenant）
- Agent A `remember(scope=private)` → Agent B `recall` 看不到
- Agent A `remember(scope=shared)` → Agent B 可发现

**公理 5 检查**：可见性规则是访问控制机制，不决定谁看什么。✅

### 回路 C-3：身份连续性（闭合"越用越好"回路）

> **断点**：Agent 注册仅存于进程内存。MCP `growth(agent_id)` → "Agent not found"。
> **Soul 冲突**：公理 9（越用越好）。
> **试用证据**：MCP `growth` 对任何 agent_id 返回 "Agent not found"。

**闭合方案**：

Agent 状态在首次访问时自动注册（lazy registration），持久化到 `{root}/agent_index.json`。
已有 `restore_agents()` 但只恢复 `Active` 状态的 agent。需要扩展为：任何 API 调用中的 `agent_id`，如果不存在则自动注册为 `Active` 状态。

```rust
fn ensure_agent_registered(&self, agent_id: &str) {
    if !self.scheduler.has_agent(agent_id) {
        let _ = self.register_agent(agent_id);
    }
}
```

每个写操作（put/remember/session_start 等）在执行前调用 `ensure_agent_registered`。

**验收标准**：
- MCP `session_start(agent_id="new-agent")` → agent 自动注册
- 重启内核 → `growth(agent_id="new-agent")` 返回使用统计（非 "not found"）
- `agents` 命令列出所有历史 agent

**公理 5 检查**：自动注册是资源分配机制（类比 Unix 的 lazy page allocation），不决定 agent 的行为。✅

### 回路 C-4：接口对称（闭合 CLI ↔ MCP 回路）

> **断点**：MCP 有 session/pipeline/delta/growth，CLI 完全没有。CLI 有 tags 命令，MCP 没有。
> **Soul 冲突**：公理 6（结构先于语言——统一接口）。
> **试用证据**：CLI `--help` 无 session/delta/growth 命令。

**闭合方案**：

CLI 补齐以下命令：

| 新增 CLI 命令 | 对应 MCP action | 用途 |
|-------------|----------------|------|
| `session-start` | session_start | 开启会话，返回 delta + warm_context |
| `session-end` | session_end | 结束会话，自动检查点 |
| `delta` | delta | 查看增量变更 |
| `growth` | growth | 查看 agent 使用统计 |
| `hybrid` | hybrid | Graph-RAG 混合检索 |

同时修复 CLI 已知 bug：
- B3：`get --cid` flag 解析修复（改为 `get <CID>` 的文档对齐）
- help 输出补充上述新命令

**验收标准**：
- CLI `session-start --agent test` 返回 session_id + delta
- CLI `delta --since-seq 0 --agent test` 返回事件列表
- CLI `growth --agent test` 返回使用统计
- CLI `hybrid "query" --agent test` 返回 Graph-RAG 结果

**公理 5 检查**：CLI 是协议适配器，不在 kernel 添加任何新逻辑。✅

### 回路 C-5：资源一致性（闭合 MCP Resource 回路）

> **断点**：`plico://skills` 返回空数组，但 `plico_skills list` 返回 6 个。`plico://delta` 返回 placeholder。
> **Soul 冲突**：公理 7（主动先于被动——资源应提供真实上下文）。
> **试用证据**：MCP `resources/read(uri="plico://skills")` → `{"skills": []}`

**闭合方案**：

```rust
// 修复 plico://skills
fn read_skills_resource(kernel: &AIKernel) -> Value {
    let skills = kernel.list_skills();  // 复用 plico_skills list 的逻辑
    json!({ "skills": skills })
}

// 修复 plico://delta（依赖 C-1 事件持久化）
fn read_delta_resource(kernel: &AIKernel) -> Value {
    let resp = kernel.handle_api_request(ApiRequest::DeltaSince {
        agent_id: "mcp-agent".to_string(),
        since_seq: 0,  // 返回最近 N 条
        watch_cids: vec![],
        watch_tags: vec![],
        limit: Some(20),
    });
    // 转换为 resource 格式
    extract_delta_from_response(resp)
}
```

**验收标准**：
- `resources/read(plico://skills)` 返回 6 个 builtin skill
- `resources/read(plico://delta)` 返回真实事件（依赖 C-1）
- `resources/read(plico://status)` 的 `cas_object_count` 显示 SemanticFS 对象数（修复 B7）

**公理 5 检查**：Resource 是只读数据投影，不触发任何行为。✅

### 回路 C-6：AI 体验验证（闭合"测试 ↔ 现实"回路）

> **断点**：368 个单元测试通过，但 8 个 bug 全部逃逸。
> **根因**：单元测试验证 API 正确性，不验证 AI 工作流连贯性。
> **试用证据**：本文档第 0 节的全部发现。

**闭合方案**：

新增 `tests/ai_experience_test.rs`——模拟真实 AI Agent 的多 session 工作流：

```rust
#[test]
fn test_ai_agent_multi_session_experience() {
    let root = tempdir();
    
    // === Session 1: Agent A 创建数据 ===
    {
        let kernel = AIKernel::new(root.path());
        let r = kernel.handle_api_request(StartSession { agent_id: "agent-a", .. });
        assert!(r.session_started.is_some());
        
        kernel.handle_api_request(Create { content: "ADR: decision 1", tags: ["shared"], agent_id: "agent-a" });
        kernel.handle_api_request(Remember { content: "insight 1", agent_id: "agent-a" });
        kernel.handle_api_request(EndSession { agent_id: "agent-a", .. });
    }
    // kernel dropped, process boundary simulated
    
    // === Session 2: Agent B 搜索 Agent A 的数据 ===
    {
        let kernel = AIKernel::new(root.path());
        let r = kernel.handle_api_request(StartSession { agent_id: "agent-b", .. });
        
        // C-1: delta 应该非空（agent-a 的操作应可见）
        assert!(!r.session_started.changes_since_last.is_empty());
        
        // C-2: Agent B 能搜到 Agent A 的 CAS 对象
        let search = kernel.handle_api_request(Search { query: "ADR", agent_id: "agent-b", .. });
        assert!(!search.results.is_empty());
        
        // C-3: Agent B 的 growth 可用
        let growth = kernel.handle_api_request(AgentUsage { agent_id: "agent-b" });
        assert!(growth.ok);
        
        kernel.handle_api_request(EndSession { agent_id: "agent-b", .. });
    }
    
    // === Session 3: Agent A 回来，验证连续性 ===
    {
        let kernel = AIKernel::new(root.path());
        let r = kernel.handle_api_request(StartSession { agent_id: "agent-a", .. });
        
        // C-1: 能看到 Agent B 的活动
        assert!(!r.session_started.changes_since_last.is_empty());
        
        // C-3: Agent A 的 recall 还在
        let recall = kernel.handle_api_request(Recall { agent_id: "agent-a" });
        assert!(recall.memory.unwrap().iter().any(|m| m.contains("insight 1")));
    }
}
```

**验收标准**：
- 该测试覆盖 C-1 到 C-5 的所有闭合点
- 测试模拟进程重启（drop kernel + re-create）
- 测试覆盖跨 agent 搜索
- 测试覆盖 session 连续性

**公理 5 检查**：测试是验证机制，不影响运行时行为。✅

---

## 3. 附带修复的已知 Bug

以下 bug 在实现 C-1 到 C-6 的过程中顺带修复：

| Bug | 修复位置 | 修复方式 |
|-----|---------|---------|
| B1 | `plico_mcp.rs` `handle_resources_read` | C-5 直接覆盖 |
| B2 | `kernel/ops/fs.rs` `semantic_search_with_time` | C-2 直接覆盖 |
| B3 | `aicli/commands/handlers/crud.rs` | 文档对齐为 positional arg |
| B4 | `plico_mcp.rs` tests | 添加 stub embedding guard |
| B5 | `kernel/event_bus.rs` | C-1 直接覆盖 |
| B6 | `bin/plico_mcp.rs` main | 添加 `tracing_subscriber::fmt::init()` |
| B7 | `kernel/ops/dashboard.rs` | 改为查询 SemanticFS 对象数 |
| B8 | `bin/plico_mcp.rs` `dispatch_cold_layer` | 在 unknown method 错误中列出可用 method |

---

## 4. Soul 2.0 对齐表

### 试用前 vs 闭合后

| 公理 | 试用时符合度 | 闭合后预期 | 关键变化 |
|------|-----------|-----------|---------|
| 1. Token 最稀缺 | 90% | **95%** | B7 修复让统计真实，CAS 计数不再误导 |
| 2. 意图先于操作 | 75% | **80%** | CLI 补齐 session 入口（C-4），但 intent 仍需更多使用验证 |
| 3. 记忆跨越边界 | 80% | **90%** | 事件持久化（C-1）让记忆跨进程可追溯 |
| 4. 共享先于重复 | **60%** | **90%** | C-2 直接修复搜索隔离，CAS 对象同 tenant 可见 |
| 5. 机制不是策略 | 100% | 100% | 不变——所有闭合回路均是机制 |
| 6. 结构先于语言 | 100% | 100% | C-4 消除 CLI/MCP 能力差集 |
| 7. 主动先于被动 | **50%** | **85%** | C-1 + C-5 让 delta/resource 提供真实数据 |
| 8. 因果先于关联 | 40% | **50%** | Node 6 不改因果机制（已有），只确保 skill 教学可用 |
| 9. 越用越好 | **50%** | **80%** | C-3 身份连续 + C-1 事件持久 = growth 可追踪 |
| 10. 会话一等公民 | **60%** | **90%** | C-1 delta + C-4 CLI session + C-3 身份 = 完整会话体验 |
| **加权总分** | **~70%** | **~87%** | |

### 公理 8（因果先于关联）为什么只到 50%

因果边需要 Agent 主动创建。OS 提供了机制（`add_edge(type=causes)`），也提供了教学（skill "knowledge-graph"）。
但让 OS 自动推断因果关系违反公理 5（机制不是策略）。
真正的因果丰富需要 AI Agent 侧的策略——比如在 put 文档时主动创建因果边。
这超出了 OS 的职责范围。Node 6 的原则是：**确保机制可用，不替 Agent 做决策。**

### 公理 5 红线检查

| 回路 | 行为 | 是机制还是策略？ |
|------|------|----------------|
| C-1 事件持久化 | 追加写入 JSONL，启动时加载 | **机制**：类比 ext4 journal |
| C-2 共享可见性 | tenant 内 CAS 对象默认可见 | **机制**：类比 Unix 同组可读 |
| C-3 身份自动注册 | 首次 API 调用 → lazy 注册 | **机制**：类比 lazy page allocation |
| C-4 CLI 补齐 | 适配器增加命令映射 | **机制**：纯协议转译 |
| C-5 资源修复 | Resource 查询内核真实数据 | **机制**：只读投影 |
| C-6 体验测试 | 模拟真实 AI 多 session 流程 | **验证机制**：不影响运行时 |

---

## 5. MVP 实施计划

### Sprint 6: 闭合回路（2 周）

> Node 6 不建新房间，而是让已有的房间全部通电。

#### 第一周：核心闭合（C-1, C-2, C-3）

| 任务 | 文件 | 验收 |
|------|------|------|
| C-1 事件日志持久化 | `src/kernel/event_bus.rs` | 写入→重启→delta 非空 |
| C-1 事件日志恢复 | `src/kernel/mod.rs` | `restore_event_log` 从 JSONL 恢复 |
| C-2 搜索共享可见性 | `src/kernel/ops/fs.rs` | Agent B 搜到 Agent A 的 CAS 对象 |
| C-3 Agent 自动注册 | `src/kernel/mod.rs` | 首次 API 调用自动注册并持久化 |
| C-3 Agent 持久化增强 | `src/kernel/persistence.rs` | 所有 agent（含 lazy 注册的）跨进程可用 |
| Bug B4 修复 | `src/bin/plico_mcp.rs` tests | stub embedding 下 recall_semantic 不 panic |
| Bug B6 修复 | `src/bin/plico_mcp.rs` main | 添加 tracing_subscriber 初始化 |

#### 第二周：接口闭合 + 验证（C-4, C-5, C-6）

| 任务 | 文件 | 验收 |
|------|------|------|
| C-4 CLI session 命令 | `src/bin/aicli/commands/` | session-start/end/delta/growth/hybrid 可用 |
| C-4 CLI bug 修复 | `src/bin/aicli/commands/handlers/crud.rs` | B3 修复 |
| C-5 skills resource 修复 | `src/bin/plico_mcp.rs` | `plico://skills` 返回 6 个 skill |
| C-5 delta resource 修复 | `src/bin/plico_mcp.rs` | `plico://delta` 返回真实事件 |
| C-5 status 计数修复 | `src/kernel/ops/dashboard.rs` | B7 修复 |
| C-5 cold-layer 错误增强 | `src/bin/plico_mcp.rs` | B8 修复 |
| C-6 AI 体验集成测试 | `tests/ai_experience_test.rs` | 多 session 跨 agent 完整流程 |

### 依赖关系

```
C-1 (事件持久化)
  │
  ├──→ C-5 (delta resource 修复，依赖事件数据)
  │
  └──→ C-6 (测试中的 delta 断言，依赖事件持久化)

C-2 (共享可见性)
  │
  └──→ C-6 (测试中的跨 agent 搜索，依赖共享可见性)

C-3 (身份连续性)
  │
  └──→ C-6 (测试中的 growth 断言，依赖 agent 持久化)

C-4 (CLI 补齐) —— 独立，可并行

C-5 (资源修复) ← 部分依赖 C-1
```

### 代码量估算

| 回路 | 新增/修改行数 | 文件数 |
|------|-------------|--------|
| C-1 | ~100 | 2 (event_bus.rs, mod.rs) |
| C-2 | ~30 | 1 (ops/fs.rs) |
| C-3 | ~50 | 2 (mod.rs, persistence.rs) |
| C-4 | ~200 | 3 (aicli commands/) |
| C-5 | ~80 | 1 (plico_mcp.rs) |
| C-6 | ~200 | 1 (tests/ai_experience_test.rs) |
| Bug fixes | ~50 | 3 |
| **合计** | **~710** | **~13** |

**特点**：零新模块，零新概念，全是修复和连接。这正是"闭合回路"的含义。

---

## 6. 本次试用的完整经验（融入 dogfood）

### 操作记录

| 步骤 | 入口 | 结果 | 发现 |
|------|------|------|------|
| 1. system-status | CLI | CAS=1, KG=132/4512 | B7: 计数不准确 |
| 2. search "adr" | CLI | 3 条 ADR | 正常 |
| 3. get <CID> | CLI | 读到 v2.2 和 v3.0 ADR | 正常，B3: --cid flag 坏 |
| 4. put 经验 | CLI | 成功，返回 CID | 写入 3.3 秒（含 KG upsert） |
| 5. remember + recall | CLI | 2 条 Working 记忆 | 正常 |
| 6. context assemble --budget 200 | CLI | **3 对象自动 L2→L0 降级** | **亮点：token budget 完美** |
| 7. L0/L1/L2 分层 | CLI | L0=15, L1=82, L2=82 tokens | 正常 |
| 8. nodes/edges | CLI | 24 nodes, 352 edges | 全是 AssociatesWith |
| 9. MCP initialize + tools/list | MCP | 3 工具返回 | 正常 |
| 10. MCP session_start | MCP | session_id + warm_context | 正常，但 delta 空 |
| 11. MCP search(agent=cursor-trial) | MCP | **0 结果** | **B2: ownership 隔离** |
| 12. MCP search(agent=plico-dev) | MCP | 4 结果 | 换 agent_id 才行 |
| 13. MCP pipeline 3 步 | MCP | **3 步全成功** | **亮点：pipeline** |
| 14. MCP resources/list | MCP | 3 资源 | 正常 |
| 15. MCP plico://skills | MCP | **空数组** | **B1** |
| 16. MCP plico_skills list | MCP | 6 个 skill | 与 B1 矛盾 |
| 17. MCP cold-layer teaching error | MCP | **返回示例** | **亮点：teaching error** |
| 18. MCP cold-layer add_node | MCP | 成功创建 entity | 正常 |
| 19. MCP delta(since_seq=0) | MCP | **空** | **B5** |
| 20. MCP growth | MCP | **Agent not found** | **B5 相关** |
| 21. cargo test --lib | test | 368 pass, 0 fail | 正常 |
| 22. cargo test --bin plico-mcp | test | 29 pass, **1 fail** | B4 |

### 亮点（Node 5 成功验证）

1. **Context Budget Assembly** — 200 token 预算下 3 对象自动 L2→L0 降级（公理 1 完美）
2. **Pipeline 批量执行** — 3 步 1 call，消除 round-trip（Node 5 v3.0 核心兑现）
3. **Teaching Error** — 冷层缺参数返回完整示例（公理 9 机制级实现）
4. **Token Estimate 全覆盖** — 每个响应都有成本透明字段
5. **3 工具 + 6 Skill** — 架构简洁，功能覆盖完整

### 痛点（Node 6 驱动力）

1. **跨进程状态断裂** — event/delta/growth 全部因持久化缺失而失效
2. **跨 agent 数据孤岛** — ownership filter 违反公理 4
3. **CLI/MCP 能力不对称** — session 命令只在 MCP 有
4. **MCP Resource 数据虚假** — skills 返回空、delta 返回 placeholder

---

## 7. Node 6 完成后的全景

```
节点 1: 家（存储）       — CAS + SemanticFS + LayeredMemory + EventBus + Tools
节点 2: 大脑（智能）     — Prefetcher + Auth + EdgeVec BQ + KG Causal + MCP + Batch
节点 3: 意识（连续性）   — Session + Delta + Checkpoint + IntentCache + Persist
节点 4: 同事（协作）     — HybridRetrieve + KnowledgeEvent + GrowthReport + TaskDelegate
节点 5: 开门（接口）     — 3 MCP Tools + Pipeline + Resources + Skills + Teaching Error
节点 6: 闭合（可靠）     — EventPersist + SharedVisibility + IdentityContinuity + CLI↔MCP Parity
```

**从 AI 第一人称**：

> Node 5 之前，我站在门外。
> Node 5 之后，我走进了房子。
> Node 6 之后，**房子里的每一盏灯都亮了，每一扇门都开着，我的名字写在门牌上。**
>
> 第 1 次会话：我 `session_start`，OS 说"欢迎回来，自从上次有 3 件事变了"。
> 第 10 次会话：我的 skill 库已经有 12 个，包括我自己创建的 6 个。
> 第 50 次会话：我的同事 Agent B 发现了我的 ADR，用它解决了一个类似问题。
> 第 100 次会话：我查 `growth`，看到我的 token 成本从第 1 次的 5,600 降到了 280。
>
> **这就是"越用越好"的真实样子。不是文档里的承诺，而是可测量的现实。**
>
> **而且，这一切不需要新概念。只需要把已有的线路接通。**

---

## 附录 A: 与 Node 5 维度 B 的关系

Node 5 维度 B（F-15 到 F-19：自适应预热、知识发现、记忆生命周期、存储治理、运营自感知）
依赖 Node 6 的闭合回路：

| Node 5 维度 B 功能 | 依赖的 Node 6 回路 | 原因 |
|-------------------|-------------------|------|
| F-15 自适应预热 | C-1 事件持久化 | 预热排序需要历史使用数据，来自 event log |
| F-16 知识发现 | C-2 共享可见性 | 发现共享知识的前提是搜索能看到其他 agent 的数据 |
| F-17 记忆生命周期 | C-3 身份连续性 | 记忆 TTL 刷新基于 agent 的 access_count，需要跨进程 |
| F-18 存储治理 | C-1 事件持久化 | cold 标记基于 last_accessed_at，需要持久的访问记录 |
| F-19 运营自感知 | C-5 资源一致性 | HealthIndicators 通过 MCP resource 暴露 |

**结论**：Node 6 是 Node 5 维度 B 的前置条件。先闭合回路，再建自治。

## 附录 B: 后续方向（post-Node 6）

| 方向 | 依赖 | 预计节点 |
|------|------|---------|
| Node 5 维度 B（F-15 ~ F-19） | Node 6 全部完成 | Node 7 |
| 语义去重（mark-and-sweep） | C-2 共享可见性 | Node 7+ |
| MCP 长连接 / daemon 模式 | C-1 事件持久化 | Node 7+ |
| 真实 embedding 集成测试 | 基础设施 | 持续 |
| SSE 适配器复合路由对齐 | Node 5 MCP 验证 | Node 7+ |

---

*文档版本: v1.0。基于 AI Agent 首次实机试用推导。
不增加新概念，不建新模块。710 行修改闭合 6 条断路。
Soul 2.0 符合度从 70% 提升到 87%。
每一个改动都通过公理 5（机制不是策略）红线检查。
验收标准：`tests/ai_experience_test.rs` 多 session 跨 agent 完整绿灯。*
