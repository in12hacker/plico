# Plico 第十节点设计文档
# 正名 — 让每个操作名副其实

**版本**: v1.0
**日期**: 2026-04-20
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Production 契约强化
**前置**: 节点 7（代谢）✅ / 节点 8（驾具）✅ / 节点 9（韧性）设计完成
**验证方法**: Dogfooding Bug 复现测试 + TDD + Agentic CLI 评分卡
**信息来源**: `docs/dogfood2.md` 12 个 Bug（B9–B20）+ Agentic CLI Design 7 原则 + MCP Error Handling 最佳实践

---

## 0. 为什么叫"正名"：从孔子到 Plico

> "名不正，则言不顺；言不顺，则事不成。" — 《论语·子路》

Dogfooding Round 2 揭示了一个系统性问题：**Plico 声明了能力，但没有履行契约**。

```
Dogfood2 暴露的"名不副实"：

  delete 说删除了 → 搜索仍返回该对象           (B9)
  hybrid search 声称 Graph-RAG → 永远返回 0     (B10)
  suspend/resume 说执行了 → agents 列表不变     (B12)
  context.load L0 说返回摘要 → 实际返回全文     (B13)
  events --agent 说按 agent 过滤 → 实际不过滤   (B14)
  10 个命令说"执行成功" → 不打印任何内容        (B15)
  get 不存在的 CID → exit 0，无错误消息         (B18)
  tool call 不存在的 tool → 空返回，无诊断       (B19)
```

### 与 Node 9 的区别

| 维度 | Node 9（韧性） | Node 10（正名） |
|------|----------------|-----------------|
| 关注点 | 已知退化模式下的恢复 | 操作语义与实际行为的一致性 |
| 典型问题 | BM25 分数低、embedding 退化 | search 返回已删除对象、命令无输出 |
| 修复方式 | Circuit Breaker、评分优化 | 数据契约修正、结构化反馈 |
| 检验标准 | 降级后自动恢复 | 每个操作的行为与声明完全一致 |

### 生物学类比

| 概念 | Node 1-8 | Node 9 | Node 10 |
|------|---------|--------|---------|
| 身体 | 所有器官就位 | 免疫系统 | **本体感知** |
| 大脑 | 能搜索、能记忆 | 在退化时仍能工作 | **知道自己在做什么** |
| 神经 | 有事件总线 | 痛觉反馈到大脑 | **每根神经末梢都接通** |
| 行为 | 能做动作 | 动作失败后能恢复 | **动作结果与意图一致** |

**本体感知（Proprioception）**：闭着眼也知道手在哪里。Node 10 让 Plico 对自身状态的报告与实际状态完全一致——AI Agent 不需要"猜"操作是否成功。

---

## 1. 链式推演：从 Dogfood2 痛点到 Node 10

### 故障链 1：数据泄漏（B9 — Critical）

```
CAS delete(cid) 执行
  → SemanticFS.delete() 从 search_index + bm25_index + tag_index 移除 ✓
  → 对象进入 recycle_bin ✓
  → 重启 Kernel
  → SemanticFS::new() 调用 rebuild_tag_index()
    → 遍历 CAS 所有对象，不检查 recycle_bin  ← 根因
  → rebuild_vector_index() 同理
  → 已删除的 CID 重新进入搜索索引
  → search 返回已删除对象

契约违反：delete 承诺"从搜索中移除"，但只在当前会话有效。
灵魂偏差：公理 3（记忆跨越边界）— 删除状态没有跨越重启边界。
```

### 故障链 2：Hybrid 管道断裂（B10 — Critical）

```
hybrid_retrieve(query) 调用
  → Step 1: vector_search(query, limit*2)
    → embedding.embed(query) → stub 返回固定向量
    → search_backend.search(fixed_vector, limit) → 0 个真实匹配
  → Step 2: KG seed expansion — 从 vector_results 提取种子
    → vector_results = [] → graph_seeds = []
  → Step 3: graph_traverse([], edge_types, depth)
    → 无种子 → 无遍历 → 0 结果
  → Step 4: Merge → 0 vector + 0 graph = 0
  → 返回 HybridResult { items: [], vector_count: 0, graph_count: 0 }

根因：hybrid pipeline 只有一条路径 (vector → KG)，无 BM25 降级路径。
对比：2026 Production RAG 标准是 "BM25 + Vector 并行 + RRF 融合"，
      hybrid 不应依赖 vector 作为唯一入口。
```

**联网校正**：
- **arXiv:2507.03226v3** (Practical GraphRAG): "cascaded retrieval strategy combining graph traversal with vector-based ranking" — 但 graph traversal 的种子不应只来自 vector，BM25 也应提供种子。
- **NetApp Hybrid RAG 2026**: "BM25 provides structure and explainability that embeddings alone cannot" — BM25 在 stub embedding 场景下是唯一可靠的召回路径。
- **DEV Community 2026 Production Standard**: "BM25 + Vector + Reranker is the 2026 production standard" — hybrid 至少需要 BM25 fallback。

### 故障链 3：静默失败链（B11 + B15 + B18 + B19）

```
Agent 调用 CLI 命令（如 delete）
  → 权限不足 → kernel 返回 Err(PermissionDenied)
  → cmd_delete() 返回 ApiResponse::error(e.to_string())
  → print_result() 检查 ApiResponse 的各字段：
    → cid: None → 不打印
    → tags: None → 不打印
    → data: None → 不打印
    → results: None → 不打印
    → error: Some("Permission denied") → 但 print_result 没有处理 error 字段！
  → CLI exit 0 → 无任何输出

同理：
  ApiResponse::ok()（无可打印字段）→ suspend/resume/terminate/remember/edge 等
  cmd_get(invalid_cid) → ApiResponse::error() → 无输出 + exit 0
  cmd_tool_call(invalid_tool) → 空结果 → 无输出

结果：AI Agent 无法区分"成功但无需输出"和"失败了但不告诉你"。
```

**联网校正**：
- **Agentic CLI Design 原则 5** (Observable & Debuggable): "Classify exit codes: 0=success / 2=argument error / 3=auth error / 4=retry recommended"
- **Agent Desktop Exit Codes**: "Exit code 0=success, 1=structured error, 2=usage error" + JSON 包含 error.code、error.suggestion
- **MCP Error Best Practice (Alpic AI 2026)**: "Tool error responses are context, not dead ends — include recovery guidance"
- **Claude Code #14419**: "Silent failures are the hardest failure mode to debug"
- **OpenAI Codex #15536**: "Silent success on actual failure is the worst pattern for CI/automation"

### 故障链 4：Agent 生命周期幻觉（B12）

```
cmd_agent_suspend(agent_id="cli") 调用
  → kernel.agent_suspend("cli")
    → scheduler.update_state(&AgentId("cli"), AgentState::Suspended)
    → persist_agents() → 写入 agents_index.json ✓
  → 返回 ApiResponse::ok()
  → print_result(): ok=true 但无可打印字段 → 无输出  ← B15
  → Agent 以为失败了
  → Agent 再次调用 agents list
  → 看到状态是... 可能仍然是 "Created"

可能的根因组合：
  a) 自动注册的 agent ID 与 CLI 使用的 agent ID 不一致
  b) scheduler 状态转换被拒绝（无效状态机路径），但错误被吞
  c) print_result 不显示操作确认 → Agent 误以为操作无效

无论哪种，核心问题是：操作结果不可观测。
```

### 故障链 5：Context Layer 不降级（B13）

```
context.load(cid, layer="L0") 调用
  → ContextLoader.load(cid, ContextLayer::L0)
  → load_l0(cid):
    → 检查 L0 缓存 → 缓存未命中
    → 检查 L0 文件 (context/l0/{shard}/{cid}) → 文件不存在
    → fallback: 从 CAS 加载全文 → compute_l0(&raw)
      → summarizer=None (无 LLM) → 启发式截断
      → 启发式：words <= 20 → 返回全文  ← 短内容直接返回
      → 启发式：words > 20 → first 10 + last 10 words
    → 但返回的 layer 字段是 ContextLayer::L0  ← 声称是 L0

问题：对长内容，"first 10 + last 10 words" 不是合格的 L0 摘要。
      对短内容（<=20 words），返回全文是合理的但 layer 声称 L0。
      用户报告 "L0 返回 layer=L2 全文内容"，可能是：
      a) CAS 中对象的全文恰好很短（<=20 words）→ 全文返回 → layer 标记 L0
      b) compute_l0 的启发式质量过低 → 内容像全文

更深层：没有 LLM Summarizer 时，L0 和 L2 的区别几乎不存在。
         系统应该诚实地报告 "L0 摘要不可用，降级为 L2"。
```

### 推导链

```
Node 1-6：构建能力栈（能做什么）
Node 7：让系统活起来（代谢、进化）
Node 8：让 Agent 容易使用（驾具、引导）
Node 9：让已有能力不失效（韧性、恢复）
Node 10：让每个操作的行为与声明一致（正名、契约）

正名 = 对外：每个操作都给出诚实的结构化反馈
      对内：每个数据契约在所有边界条件下都成立
      元层：系统能准确报告自身的能力边界和退化状态
```

---

## 2. Node 10 的三个维度

### 维度 A：数据契约 — 让操作语义在所有边界条件下成立

#### F-43: 软删除搜索隔离

**问题**: B9 — search 返回已软删除的对象。

**根因**: `rebuild_tag_index()` 和 `rebuild_vector_index()` 遍历 CAS 所有对象时不排除 recycle_bin 中的 CID。

**修复**:

```rust
// src/fs/semantic_fs/mod.rs — rebuild_tag_index
fn rebuild_tag_index(cas: &CASStorage) -> HashMap<String, Vec<String>> {
    let bin = Self::load_recycle_bin_static(&cas.root().join("../recycle_bin.json"))
        .unwrap_or_default();
    let mut index: HashMap<String, Vec<String>> = HashMap::new();
    for cid in cas.list_cids() {
        if bin.contains_key(&cid) { continue; } // 排除已删除
        if let Ok(obj) = cas.get_raw(&cid) {
            for tag in &obj.meta.tags {
                index.entry(tag.clone()).or_default().push(cid.clone());
            }
        }
    }
    index
}

// rebuild_vector_index 同理
fn rebuild_vector_index(&self) {
    let bin = self.recycle_bin.read().unwrap();
    for cid in self.cas.list_cids() {
        if bin.contains_key(&cid) { continue; } // 排除已删除
        if let Ok(obj) = self.cas.get_raw(&cid) {
            self.upsert_semantic_index(&cid, &obj.data, &obj.meta);
        }
    }
}
```

**验证标准**:
1. `put` → `delete` → 重启 kernel → `search` 不返回该 CID
2. `put` → `delete` → `restore` → `search` 返回该 CID
3. 性能：rebuild 时间不退化（recycle_bin 查找 O(1)）

**公理 3 对齐**: 删除语义跨越重启边界 ✅

---

#### F-44: Hybrid Search BM25 降级路径

**问题**: B10 — hybrid search 在 stub embedding 下永远返回 0 结果。

**根因**: `hybrid_retrieve()` 的 Step 2（KG seed expansion）只从 vector 结果中提取种子。当 vector 返回空（stub embedding），整个 pipeline 断裂。

**修复**: 引入 BM25 → KG 的备用种子路径。

```rust
// src/kernel/ops/hybrid.rs — hybrid_retrieve
pub fn hybrid_retrieve(/* ... */) -> HybridResult {
    // Step 1: Vector search (may return 0 under stub embedding)
    let vector_results = self.vector_search(query_text, max_results * 2);

    // Step 1b: BM25 search (always available)
    let bm25_results = self.fs.bm25_search(query_text, max_results * 2);

    // Step 2: KG seed expansion — from vector OR bm25 results
    let mut graph_seeds: Vec<(String, f32)> = Vec::new();
    if let Some(ref kg) = self.knowledge_graph {
        // Primary: vector results
        for hit in &vector_results {
            if let Ok(Some(node)) = kg.get_node(&hit.cid) {
                graph_seeds.push((node.id.clone(), hit.score));
            }
        }
        // Fallback: BM25 results (when vector yields nothing)
        if graph_seeds.is_empty() {
            for (cid, score) in &bm25_results {
                if let Ok(Some(node)) = kg.get_node(cid) {
                    graph_seeds.push((node.id.clone(), *score));
                }
            }
        }
    }

    // Step 3: Graph traversal (now has seeds even without vector)
    let (graph_hits, path_count) = self.graph_traverse(&graph_seeds, edge_types, graph_depth);

    // Step 4: Merge — use RRF across all three sources
    // ... (existing merge logic, extended to include bm25_results)
}
```

**设计决策**: BM25 种子是 fallback 而非并行路径，因为 BM25 分数和 vector 分数不可直比。当 vector 有结果时，BM25 种子被跳过以避免噪声。这符合 arXiv:2507.03226v3 的 "cascaded retrieval" 模式。

**验证标准**:
1. `EMBEDDING_BACKEND=stub` → `hybrid --query "test"` → 返回 >0 结果
2. 有真实 embedding 时 → hybrid 行为不变（BM25 fallback 不触发）
3. BM25 种子 → KG 扩展路径正确工作

**公理 2 对齐**: 意图（"搜索相关知识"）在 embedding 退化时仍可达成 ✅

---

#### F-45: Agent 生命周期状态机验证

**问题**: B12 — suspend/resume/terminate 返回 exit 0 但 agents 列表始终 "Created"。

**修复**: 三层加固。

```rust
// Layer 1: 状态转换后验证
pub fn agent_suspend(&self, agent_id: &str) -> std::io::Result<()> {
    let aid = AgentId(agent_id.to_string());
    // ... existing logic ...

    self.scheduler.update_state(&aid, AgentState::Suspended).map_err(transition_err)?;
    self.persist_agents();

    // 验证：读回持久化数据确认状态
    let verified = self.scheduler.get(&aid)
        .map(|a| format!("{:?}", a.state()))
        .unwrap_or_else(|| "unknown".to_string());
    tracing::info!("Agent {} state verified: {}", agent_id, verified);

    Ok(())
}

// Layer 2: CLI handler 返回确认信息
pub fn cmd_agent_suspend(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_suspend(&agent_id) {
        Ok(()) => {
            let mut r = ApiResponse::ok();
            r.message = Some(format!("Agent '{}' suspended", agent_id));
            // 附带当前状态确认
            if let Some(agent) = kernel.get_agent(&agent_id) {
                r.data = Some(format!("state: {:?}", agent.state()));
            }
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

// Layer 3: persist_agents 写入后 fsync 确保落盘
fn persist_agents(&self) {
    let agents = self.scheduler.snapshot_agents();
    atomic_write_json_sync(&self.agent_index_path(), &agents); // fsync
}
```

**验证标准**:
1. `agent --register test-agent` → `suspend --agent test-agent` → `agents` 显示 "Suspended"
2. 重启后 `agents` 仍显示 "Suspended"
3. 无效转换（如 Created → Resume）返回明确错误

**公理 3 对齐**: 生命周期状态跨越会话边界 ✅

---

#### F-46: Context Layer 诚实降级

**问题**: B13 — `context.load L0` 不降级，请求 L0 返回 L2 全文内容。

**根因**: 无 LLM Summarizer 时，`compute_l0` 启发式质量低。系统不报告降级事实。

**修复**: 诚实报告实际返回的内容层级。

```rust
pub struct LoadedContext {
    pub cid: String,
    pub requested_layer: ContextLayer,   // Agent 请求的层级
    pub actual_layer: ContextLayer,      // 实际返回的层级
    pub content: String,
    pub tokens_estimate: usize,
    pub degraded: bool,                  // 是否降级
    pub degradation_reason: Option<String>,
}

fn load_l0(&self, cid: &str) -> std::io::Result<LoadedContext> {
    // 1. 尝试从缓存/磁盘加载预计算的 L0
    if let Some(content) = self.l0_cache.read().unwrap().get(cid).cloned() {
        return Ok(LoadedContext {
            cid: cid.to_string(),
            requested_layer: ContextLayer::L0,
            actual_layer: ContextLayer::L0,
            content, tokens_estimate: /* ... */,
            degraded: false,
            degradation_reason: None,
        });
    }

    // 2. 无预计算的 L0 → 尝试计算
    let raw = self.cas.get(cid)
        .map(|obj| String::from_utf8_lossy(&obj.data).into_owned())
        .unwrap_or_default();

    let token_count = raw.split_whitespace().count() * 3 / 4;

    if token_count <= 100 {
        // 内容本身就在 L0 范围内 → 返回全文，标记为 L0
        return Ok(LoadedContext {
            actual_layer: ContextLayer::L0,
            degraded: false,
            /* ... */
        });
    }

    if self.summarizer.is_some() {
        // 有 LLM → 生成真正的 L0 摘要
        let summary = self.compute_l0(&raw);
        return Ok(LoadedContext {
            actual_layer: ContextLayer::L0,
            degraded: false,
            content: summary,
            /* ... */
        });
    }

    // 无 LLM 且内容超过 L0 范围 → 诚实降级
    let heuristic = self.compute_l0(&raw);
    Ok(LoadedContext {
        cid: cid.to_string(),
        requested_layer: ContextLayer::L0,
        actual_layer: ContextLayer::L2, // 诚实标记
        content: heuristic,
        tokens_estimate: heuristic.split_whitespace().count() * 3 / 4,
        degraded: true,
        degradation_reason: Some(
            "No LLM summarizer available; returned heuristic truncation".into()
        ),
    })
}
```

**验证标准**:
1. 有 LLM Summarizer → `context.load L0` 返回 `actual_layer: L0, degraded: false`
2. 无 LLM Summarizer + 长内容 → 返回 `actual_layer: L2, degraded: true, degradation_reason: "..."`
3. 内容 <=100 tokens → 返回 `actual_layer: L0, degraded: false`（短内容全文就是摘要）

**公理 1 对齐**: Agent 根据 `degraded` 标记决定是否消费该内容，避免 token 浪费 ✅

---

### 维度 B：结构化反馈 — 让每个操作都"有声有色"

#### F-47: 统一响应信封

**问题**: B15 — 10 个 CLI 命令（delete/restore/suspend/resume/terminate/session-end/remember/edge/status/quota）执行后完全无输出。

**根因**: `print_result()` 只处理有特定数据的字段（cid、data、results、agents 等）。对于 `ApiResponse::ok()` 没有这些字段时，什么都不打印。

**Agentic CLI 原则对齐**:

| Agentic CLI 原则 | Plico 当前 | Node 10 目标 |
|-----------------|-----------|-------------|
| 原则 1: Machine-readable | ⚠️ JSON 模式有，human 模式缺 | 统一信封 |
| 原则 5: Observable | ❌ 10 命令无输出 | 每个命令有确认 |
| 原则 5: Exit code 分类 | ❌ 全部 exit 0 | 0/1/2 分类 |

**修复**: 为 `ApiResponse` 添加统一的 `message` 和 `error` 输出路径。

```rust
#[derive(Serialize, Deserialize)]
pub struct ApiResponse {
    pub ok: bool,
    pub message: Option<String>,       // 操作确认/描述
    pub error_code: Option<String>,    // 机器可读错误码
    pub fix_hint: Option<String>,      // 修复建议
    pub next_actions: Option<Vec<String>>, // 下一步建议
    // ... existing fields ...
}

impl ApiResponse {
    pub fn ok_with_message(msg: impl Into<String>) -> Self {
        let mut r = Self::ok();
        r.message = Some(msg.into());
        r
    }

    pub fn error_with_diagnosis(
        msg: impl Into<String>,
        code: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        let mut r = Self::error(msg.into());
        r.error_code = Some(code.into());
        r.fix_hint = Some(fix.into());
        r
    }
}

// print_result 补充输出
pub fn print_result(response: &ApiResponse) {
    if std::env::var("AICLI_OUTPUT").as_deref().ok() == Some("json") {
        println!("{}", serde_json::to_string_pretty(response).unwrap_or_default());
        return;
    }

    // 错误输出（最高优先级）
    if !response.ok {
        if let Some(ref err) = response.error {
            eprintln!("Error: {}", err);
        }
        if let Some(ref code) = response.error_code {
            eprintln!("  Code: {}", code);
        }
        if let Some(ref fix) = response.fix_hint {
            eprintln!("  Fix: {}", fix);
        }
        std::process::exit(1); // 非零退出码
    }

    // 操作确认
    if let Some(ref msg) = response.message {
        println!("{}", msg);
    }

    // ... existing field printing ...

    // 兜底：如果没有任何字段被打印，至少打印 "ok"
    if response.message.is_none()
        && response.cid.is_none()
        && response.data.is_none()
        && response.results.is_none()
        && response.agents.is_none()
        && response.memory.is_none()
    {
        println!("ok");
    }
}
```

**所有 10 个静默命令的修复**:

| 命令 | 当前输出 | 修复后输出 |
|------|---------|-----------|
| delete | (空) | `Deleted: {cid} → recycle bin` |
| restore | (空) | `Restored: {cid} from recycle bin` |
| suspend | (空) | `Agent '{id}' suspended (was: Running)` |
| resume | (空) | `Agent '{id}' resumed → Waiting` |
| terminate | (空) | `Agent '{id}' terminated` |
| session-end | (空) | `Session '{sid}' ended. Checkpoint: {cid}` |
| remember | (空) | `Stored memory for agent '{id}' (tags: [...])` |
| edge | (空) | `Edge created: {from} —[{type}]→ {to}` |
| status | (空) | `Agent '{id}': state={state}, memories={n}` |
| quota | (空) | `Quota — memory: {used}/{max}, cpu: {used}/{max}` |

**验证标准**:
1. 每个命令在 human-readable 模式下有 ≥1 行输出
2. 每个命令在 JSON 模式下完整返回所有字段
3. 错误情况 exit 1 + eprintln 到 stderr

---

#### F-48: 结构化错误诊断

**问题**: B11（delete 无权限静默失败）、B18（get 不存在的 CID exit 0）、B19（tool call 不存在的 tool 空返回）。

**Agentic CLI 原则对齐**:

> "Agents will determine the 'next step' from error messages. Structured exit codes and errors are crucial."
> — Agentic CLI Design, Principle 5

> "Tool error responses are context, not dead ends — include recovery guidance."
> — Alpic AI, MCP Error Handling 2026

**MCP 层修复**（`plico_mcp.rs`）:

```rust
fn dispatch_plico_action(action: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    // 权限检查失败 → 结构化诊断
    if let Err(e) = check_read_only(action, PLICO_ACTIONS) {
        return Err(serde_json::json!({
            "error": e.to_string(),
            "error_code": "PERMISSION_DENIED",
            "fix": format!(
                "Grant permission first: plico(action=\"permission\", params={{\"method\": \"grant\", \"action\": \"{}\"}})",
                action
            ),
            "next_actions": [
                format!("plico(action=\"permission\", params={{\"method\": \"grant\", \"action\": \"{}\"}})", action),
                "plico(action=\"help\")"
            ]
        }).to_string());
    }

    // CID 不存在 → 结构化诊断
    // (在 get/update/delete 路径中)
    if action == "get" {
        let cid = args.get("cid").and_then(|v| v.as_str()).unwrap_or("");
        if cid.is_empty() {
            return Err(serde_json::json!({
                "error": "CID is required",
                "error_code": "MISSING_PARAM",
                "fix": "Provide a valid CID: plico(action=\"get\", cid=\"sha256-...\")",
                "next_actions": ["plico(action=\"search\", query=\"...\")"]
            }).to_string());
        }
    }
    // ...
}
```

**错误码体系**:

| 错误码 | 含义 | CLI Exit Code | 恢复建议 |
|--------|------|-------------|---------|
| `OK` | 成功 | 0 | — |
| `NOT_FOUND` | CID/Agent 不存在 | 1 | "Run search to find valid CIDs" |
| `PERMISSION_DENIED` | 权限不足 | 1 | "Grant permission with..." |
| `INVALID_PARAM` | 参数错误 | 2 | "Check action help: plico(action=help)" |
| `INVALID_STATE` | 状态转换无效 | 1 | "Current state is X, valid transitions: [Y, Z]" |
| `TOOL_NOT_FOUND` | 工具不存在 | 1 | "Available tools: [...], use tool list" |
| `DEGRADED` | 功能降级 | 0 | "Operating in degraded mode: ..." |
| `INTERNAL` | 内部错误 | 1 | "Retry or report bug" |

**验证标准**:
1. `delete --cid invalid` → exit 1 + `error_code: NOT_FOUND` + fix hint
2. `delete --cid valid` (无权限) → exit 1 + `error_code: PERMISSION_DENIED` + grant 命令
3. `tool call --name invalid` → exit 1 + `error_code: TOOL_NOT_FOUND` + 工具列表
4. JSON 模式下所有错误包含 `error_code` + `fix` + `next_actions`

---

### 维度 C：查询契约 — 让过滤和展示逻辑言行一致

#### F-49: Events Agent 过滤修正

**问题**: B14 — `events history --agent plico-dev` 仍返回其他 agent 的事件。

**根因分析**: 需要审计 `list_events` 的 `agent_id` 过滤逻辑。

```rust
// 修复：严格按 agent_id 过滤
pub fn list_events_filtered(
    &self,
    agent_filter: Option<&str>,
    event_types: &[String],
    limit: usize,
) -> Vec<KernelEvent> {
    self.event_bus.list_events()
        .into_iter()
        .filter(|e| {
            // agent_id 过滤：Some("x") → 只返回 agent_id=="x" 的事件
            if let Some(filter_agent) = agent_filter {
                match e.agent_id() {
                    Some(eid) => eid == filter_agent,
                    None => false, // 无 agent_id 的系统事件不返回
                }
            } else {
                true
            }
        })
        .filter(|e| {
            event_types.is_empty() || event_types.contains(&e.event_type().to_string())
        })
        .take(limit)
        .collect()
}
```

**验证标准**:
1. Agent A 产生 3 个事件，Agent B 产生 2 个事件
2. `events history --agent A` → 只返回 A 的 3 个事件
3. `events history` → 返回全部 5 个事件

---

#### F-50: Tool Schema 完整暴露

**问题**: B16 — `tool describe` 只显示名称和描述，不显示参数 schema。

**修复**:

```rust
// BuiltinTool 扩展
pub struct BuiltinTool {
    pub name: String,
    pub description: String,
    pub parameters: Vec<ToolParam>,     // 参数列表
    pub returns: String,                // 返回值描述
    pub example: Option<String>,        // 示例调用
}

pub struct ToolParam {
    pub name: String,
    pub param_type: String,     // "string" | "number" | "boolean" | "array"
    pub required: bool,
    pub description: String,
    pub default: Option<String>,
}

// cmd_tool_describe 输出
fn cmd_tool_describe(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let name = extract_arg(args, "--name").unwrap_or_default();
    match kernel.get_tool(&name) {
        Some(tool) => {
            let mut r = ApiResponse::ok();
            r.data = Some(serde_json::json!({
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "type": p.param_type,
                    "required": p.required,
                    "description": p.description,
                    "default": p.default,
                })).collect::<Vec<_>>(),
                "returns": tool.returns,
                "example": tool.example,
            }).to_string());
            r
        }
        None => ApiResponse::error_with_diagnosis(
            format!("Tool '{}' not found", name),
            "TOOL_NOT_FOUND",
            format!("Available tools: {}", kernel.list_tool_names().join(", ")),
        ),
    }
}
```

**验证标准**: `tool describe --name plico` 输出包含完整参数列表和类型信息

---

#### F-51: Edge Type 命名规范化

**问题**: B17 — `explore` 显示 `relatedto` 而非 `related_to`。

**修复**: 统一所有 edge type 为 `snake_case`。

```rust
impl KGEdgeType {
    pub fn display_name(&self) -> &'static str {
        match self {
            KGEdgeType::RelatedTo => "related_to",
            KGEdgeType::DependsOn => "depends_on",
            KGEdgeType::Causes => "causes",
            KGEdgeType::PartOf => "part_of",
            KGEdgeType::Supersedes => "supersedes",
            KGEdgeType::Custom(s) => s.as_str(),
        }
    }
}

// 序列化时统一使用 display_name()
impl Serialize for KGEdgeType {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.display_name())
    }
}
```

**验证标准**: `explore --cid ...` 输出的所有 edge type 均为 snake_case

---

#### F-52: Growth 统计实时性

**问题**: B20 — `growth` 显示 `Sessions: 0`（刚 session-start 过）。

**根因**: `growth` 从持久化的 session 历史读取，但 `session_start` 可能没有写入 session 记录。

**修复**: 确保 `session_start` 创建的 session 记录在 `growth` 中可见。

```rust
pub fn cmd_growth(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let stats = kernel.get_growth_stats();
    let mut r = ApiResponse::ok();
    r.data = Some(serde_json::json!({
        "objects": stats.total_objects,
        "memories": stats.total_memories,
        "sessions": stats.active_sessions + stats.completed_sessions, // 包括活跃 session
        "active_sessions": stats.active_sessions,
        "agents": stats.agent_count,
        "kg_nodes": stats.kg_nodes,
        "kg_edges": stats.kg_edges,
    }).to_string());
    r
}
```

**验证标准**: `session-start` → `growth` 显示 `Sessions: >=1`, `active_sessions: 1`

---

## 3. 灵魂偏差检测

### 公理 5 红线检查

| Feature | 声明 | 检查 | 结果 |
|---------|------|------|------|
| F-43 软删除隔离 | 索引重建排除 recycle_bin | 纯数据过滤，无策略判断 | ✅ 机制 |
| F-44 Hybrid BM25 fallback | BM25 作为备用种子源 | 算法降级路径，无策略判断 | ✅ 机制 |
| F-45 生命周期验证 | 状态转换后验证 + 确认输出 | 状态机 + 反馈，无策略判断 | ✅ 机制 |
| F-46 Context 诚实降级 | 报告实际层级和降级原因 | 数据标注，无策略判断 | ✅ 机制 |
| F-47 统一响应信封 | 每个命令有确认输出 | 输出格式，无策略判断 | ✅ 机制 |
| F-48 错误诊断 | error_code + fix_hint | 结构化反馈，不决定如何修复 | ✅ 机制 |
| F-49 Events 过滤 | 严格按 agent_id 过滤 | 查询语义修正，无策略判断 | ✅ 机制 |
| F-50 Tool Schema | 暴露参数类型和描述 | 元数据暴露，无策略判断 | ✅ 机制 |
| F-51 命名规范化 | snake_case 统一 | 序列化格式，无策略判断 | ✅ 机制 |
| F-52 Growth 实时性 | 包含活跃 session | 数据统计修正，无策略判断 | ✅ 机制 |

**10/10 通过。** 全部特性是机制层修正，不引入任何策略。

---

## 4. Soul 2.0 对齐表

| 公理 | Node 9 后 | Node 10 后 | 变化 |
|------|-----------|------------|------|
| 1 Token 最稀缺 | 96% | **97%** | F-47 减少重试浪费，F-48 错误诊断避免盲探 |
| 2 意图先于操作 | 95% | 95% | — |
| 3 记忆跨越边界 | 96% | **98%** | F-43 删除跨重启，F-45 生命周期跨重启 |
| 4 因果胜过关联 | 92% | 92% | — |
| 5 机制不是策略 | 97% | **98%** | 10 个新特性全部是机制修正 |
| 6 结构先于语言 | 95% | **97%** | F-47 统一信封，F-48 结构化错误码 |
| 7 主动先于被动 | 96% | **97%** | F-48 错误时主动给出修复建议和 next_actions |
| **综合** | **95%** | **96.3%** | **+1.3%** |

---

## 5. Harness Engineering 对齐

Node 10 的设计直接受 Harness Engineering 思想影响，但以 Plico 的 Soul 为核心：

| Harness 原则 | Plico Node 10 实现 | 核心差异 |
|-------------|-------------------|---------|
| Observable & Debuggable | F-47 统一响应 + F-48 错误诊断 | Harness 面向 DevOps pipeline；Plico 面向 AI Agent 的 tool call 反馈 |
| Exit Code 分类 | F-48 error_code 体系 (0/1/2) | Harness 用 HTTP status；Plico 用 MCP isError + JSON error_code |
| Guides (Feedforward) | F-48 fix_hint + next_actions | Harness 是静态文档；Plico 的 fix_hint 是动态的、基于上下文的 |
| Safety Rails | F-45 状态机验证 | Harness 强调 readonly/confirm；Plico 补充状态一致性验证 |
| Self-description | F-50 Tool Schema | Harness 有 Registry；Plico 已有 ActionRegistry (F-31)，F-50 补充参数级描述 |

**Harmless 对齐**：Node 10 的核心 harmless 策略是**消除静默失败**。一个对 Agent 无害的系统不是"不出错"的系统，而是"出错时明确告知"的系统。B11 证明了静默失败比显式错误更有害——Agent 以为删除成功，实际没有执行，这可能导致数据泄漏或决策错误。

---

## 6. Agentic CLI 评分卡

**基于 Agentic CLI Design 7 原则打分（0-2 分制）**：

| 原则 | Node 9 后 | Node 10 后 | 说明 |
|------|-----------|------------|------|
| P1: Machine-readable | 1 | **2** | JSON 模式完整 + error_code + fix_hint |
| P2: HATEOAS (next_actions) | 0 | **1** | F-48 添加 next_actions（静态模板，非动态生成） |
| P3: Self-describing | 1 | **2** | F-50 tool schema + F-31 ActionRegistry |
| P4: Context-efficient | 2 | 2 | select/preview/token_budget 已实现 |
| P5: Observable & Debuggable | 0 | **2** | exit code 分类 + 错误诊断 + 操作确认 |
| P6: Idempotent-safe | 1 | 1 | CAS 天然幂等；lifecycle 操作需进一步 |
| P7: Non-destructive | 2 | 2 | 软删除 + 回收站已实现 |
| **总分** | **7/14** | **12/14** | **+5 分 (+71%)** |

---

## 7. MVP 实施计划

### Sprint 1：数据契约修正（F-43 + F-44）

**目标**: 消除 B9 和 B10 两个 Critical bug。

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/fs/semantic_fs/mod.rs` | rebuild_tag_index 排除 recycle_bin | ~15 |
| `src/fs/semantic_fs/mod.rs` | rebuild_vector_index 排除 recycle_bin | ~10 |
| `src/kernel/ops/hybrid.rs` | hybrid_retrieve 添加 BM25 fallback 种子 | ~30 |
| `tests/node10_rectification_test.rs` | B9/B10 复现测试 | ~80 |

**验收**: `cargo test` 通过 + B9/B10 复现测试绿灯

### Sprint 2：结构化反馈（F-47 + F-48）

**目标**: 消除所有静默失败（B11 + B15 + B18 + B19）。

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/api/semantic.rs` | ApiResponse 新增 message/error_code/fix_hint/next_actions | ~30 |
| `src/bin/aicli/commands/mod.rs` | print_result 处理 error + message + 兜底 "ok" | ~40 |
| `src/bin/aicli/commands/handlers/*.rs` | 10 个命令返回 ok_with_message | ~50 |
| `src/bin/plico_mcp.rs` | 错误路径返回结构化 JSON | ~40 |
| `tests/node10_rectification_test.rs` | 静默失败复现测试 | ~60 |

**验收**: 所有 37 个 CLI 命令无一静默 + 错误有 error_code

### Sprint 3：查询契约 + 生命周期（F-45 + F-46 + F-49 + F-50 + F-51 + F-52）

**目标**: 修复 B12, B13, B14, B16, B17, B20。

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/kernel/ops/agent.rs` | 状态转换后验证 + 确认信息 | ~30 |
| `src/fs/context_loader.rs` | LoadedContext 新增 degraded 字段 | ~40 |
| `src/kernel/event_bus.rs` | list_events agent_id 过滤修正 | ~15 |
| `src/kernel/builtin_tools.rs` | ToolParam 结构 + describe 输出 | ~50 |
| `src/fs/graph/*.rs` | edge type display_name() snake_case | ~10 |
| `src/kernel/ops/dashboard.rs` | growth 包含 active_sessions | ~10 |
| `tests/node10_rectification_test.rs` | 剩余 bug 复现测试 | ~80 |

**验收**: `cargo test` 全绿 + 全部 12 个 bug 有对应复现测试

### 依赖关系

```
F-47 (响应信封) ← 无依赖，最先做
  ↓
F-48 (错误诊断) ← 依赖 F-47 的信封结构
  ↓ 并行 ↙
F-43 (搜索隔离)     F-45 (生命周期)     F-49 (事件过滤)
F-44 (Hybrid 降级)  F-46 (Context 降级)  F-50 (Tool Schema)
                                          F-51 (命名规范化)
                                          F-52 (Growth 实时)
```

### 代码量估算

| 类别 | 行数 |
|------|------|
| 新增代码 | ~350 |
| 修改代码 | ~150 |
| 新增测试 | ~300 |
| **总计** | **~800 行** |

### 新增外部依赖

**零。** 所有修复基于现有 Rust std + serde_json。

---

## 8. Node 10 完成后的全景

> *以下是一个 AI Agent 在 Node 10 完成后的第一人称体验：*
>
> 我连接到 Plico，调用 `session-start`。系统返回 handover 上下文。
>
> 我尝试搜索昨天的 bug 报告：`search --query "auth failure"`。
> 返回 3 条结果，每条有 snippet 预览和相关度评分。
> 昨天我删除了一条过时的 bug 报告 — **它不在搜索结果中**。正确。
>
> 我执行 `hybrid --query "authentication" --depth 2`。
> 即使 embedding 后端是 stub，BM25 提供了 2 个种子 → KG 扩展出 5 条相关知识。
> **Hybrid 不再是空壳了。**
>
> 我想暂停一个 agent：`suspend --agent helper`。
> 系统回复：`Agent 'helper' suspended (was: Running)`。
> 我确认：`agents` — 看到 state: Suspended。重启后再查 — **仍然是 Suspended**。
>
> 我误操作，尝试删除一个我没有权限删的对象：
> ```
> Error: Permission denied for action 'delete'
>   Code: PERMISSION_DENIED
>   Fix: Grant permission first: plico(action="permission", params={"method": "grant", "action": "delete"})
> ```
> 我不需要猜。**错误告诉我下一步该做什么。**
>
> 我调用 `context.load --cid abc --layer L0`。
> 系统返回：`actual_layer: L2, degraded: true, reason: "No LLM summarizer"`。
> 我知道这不是 L0 摘要。**系统对我坦诚。**
>
> 我查看 `events history --agent helper` — 只看到 helper 的事件，不混杂其他 agent 的噪声。
>
> **这就是"正名"。不是新能力，而是已有的每一个能力都言行一致。**
> **系统对我说"删了"，它就真的删了。**
> **系统对我说"成功"，它就真的成功了，而且告诉我成功了什么。**
> **系统对我说"失败"，它告诉我为什么，以及下一步该怎么做。**

---

## 9. 后续方向（post-Node 10）

| 方向 | 依赖 | 预计节点 |
|------|------|---------|
| F-26 记忆压缩（LLM Summarizer） | F-46 诚实降级基础 | Node 11 |
| 被动知识提取（session_end 自动摘要） | F-26 + KG 因果链 | Node 11 |
| MCP HATEOAS 动态 next_actions | F-48 静态 next_actions 基础 | Node 11 |
| Cross-encoder Reranker | F-44 Hybrid BM25 path | Node 12 |
| MCP Gateway（多 Agent 路由） | F-45 生命周期可信 | Node 12 |
| 分布式 CAS（多节点复制） | F-43 数据契约完整 | Node 13+ |

---

## 附录 A: Dogfood2 Bug → Node 10 Feature 追溯

| Bug | 严重度 | 描述 | 根因 | Feature | 状态 |
|-----|--------|------|------|---------|------|
| B9 | Critical | search 返回已删除对象 | rebuild 不排除 recycle_bin | **F-43** | 待实现 |
| B10 | Critical | hybrid 永远 0 结果 | 无 BM25 fallback 种子 | **F-44** | 待实现 |
| B11 | High | delete 无权限静默失败 | print_result 不处理 error | **F-47 + F-48** | 待实现 |
| B12 | High | Agent 生命周期不持久化 | 状态确认不可观测 | **F-45 + F-47** | 待实现 |
| B13 | Medium | context.load L0 不降级 | 无降级标记 | **F-46** | 待实现 |
| B14 | Medium | events --agent 不过滤 | 过滤逻辑缺失/错误 | **F-49** | 待实现 |
| B15 | Medium | 10 个命令无输出 | ApiResponse 无 message 字段 | **F-47** | 待实现 |
| B16 | Low | tool describe 无参数 schema | BuiltinTool 缺少 parameters | **F-50** | 待实现 |
| B17 | Low | explore 显示 relatedto | KGEdgeType 序列化不一致 | **F-51** | 待实现 |
| B18 | Low | get invalid CID exit 0 | 错误不输出到 stderr | **F-48** | 待实现 |
| B19 | Low | tool call invalid exit 0 | 空返回无诊断 | **F-48** | 待实现 |
| B20 | Low | growth Sessions: 0 | 不计活跃 session | **F-52** | 待实现 |

**覆盖率**: 12/12 bug 全部有对应 Feature。零无来源特性。

---

## 附录 B: 联网技术校正记录

| 技术点 | 查证来源 | 关键事实 |
|--------|---------|---------|
| Agentic CLI Exit Code 分类 | DEV Community 2026 "Agentic CLI Design 7 Principles" | 0=success / 2=arg error / 3=auth / 4=retry |
| Agent Desktop Exit Codes | Mintlify Agent Desktop 2026 | 0=ok + JSON ok:true / 1=structured error + JSON ok:false / 2=usage |
| MCP isError vs protocol error | MCP Protocol Spec 2025-03-26, ChatForest 2026 | Tool 失败用 isError:true（result 路径），不用 JSON-RPC error |
| MCP fix_hint 模式 | Alpic AI 2026 "Better MCP Error Responses" | 错误中包含修复建议让 AI 自修正，提升 task completion rate |
| CLI HATEOAS next_actions | JoelClaw 2026 "CLI Design for AI Agents" | 每个响应包含 next_actions 模板，让 Agent 知道下一步 |
| Silent failure 危害 | Claude Code #14419, Codex #15536 | "Silent success on actual failure is the hardest mode to debug" |
| Hybrid RAG BM25 fallback | arXiv:2507.03226v3, NetApp 2026 | Graph traversal 种子应来自 BM25+Vector，非单一 vector |
| Circuit Breaker for Agent tools | AgentPatterns.ai 2026, Zylos Research 2026 | Per-tool state machine, 40-60% token savings preventing retry loops |
| Production RAG 2026 标准 | DEV Community "Beyond Basic RAG" 2026 | BM25 + Vector + Reranker 是 2026 生产标准 |

---

## 附录 C: 与先前 Node 的关系图

```
Node 1-6 (Foundation)
  ↓ 构建能力栈
Node 7 (代谢) ← 让系统活起来
  ↓ 内部机制就绪
Node 8 (驾具) ← 让 Agent 容易使用
  ↓ 接口就绪
Node 9 (韧性) ← 让已有能力不失效
  ↓ 降级可恢复
Node 10 (正名) ← 让每个操作名副其实    ← 你在这里
  ↓ 契约可信
Node 11+ (进化) ← 记忆压缩 / 被动提取 / 动态引导
```

**Node 10 的独特位置**: 它是 Node 9（韧性）的互补面。Node 9 解决"系统降级时如何恢复"，Node 10 解决"系统在所有条件下是否诚实报告自身行为"。两者共同构成**可信赖系统**的两个支柱：恢复力 + 诚信。

---

*文档版本: v1.0。基于 `docs/dogfood2.md` 12 个 bug + Agentic CLI Design 7 原则 + MCP Error Handling 最佳实践 + 9 项联网技术校正。
三个维度（数据契约 + 结构化反馈 + 查询契约），10 个特性（F-43 到 F-52），~800 行代码。
Soul 2.0 符合度从 95% 提升到 96.3%。最大提升：公理 3(+2% 跨重启契约) 和公理 6(+2% 结构化反馈)。
Agentic CLI 评分从 7/14 提升到 12/14（+71%）。
零新增外部依赖。每个 Feature 直接追溯到至少一个 Dogfood2 Bug——没有无来源的特性。*
