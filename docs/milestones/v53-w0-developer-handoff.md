# v53 W0：Rolling Hygiene（开发组任务单）

- 状态：**READY FOR DEVELOPMENT**
- 日期：2026-08-18
- 基线（开发从此 tag/branch 开工，不得从其他点）：tag
  `v53-r4-accepted-ba81b0d602b5c6298300200485894d1037951c49`（branch
  `v53-w0-hygiene`，首提交为本任务单）
- 范围来源：`docs/wheels-audit-2026-08-18-acceptance.md` W0 行 +
  `docs/wheels-audit-2026-08-18.md` 对应条目；锚点已由架构组逐条抽查核实
- 验收：轻量架构 review（diff 审计 + 定向原始摘要），**开发组不得自签验收**；
  触碰本单之外的任何文件需先提交 Architecture Deviation

## 1. 任务（五项，全部为"零风险热身"级）

### W-06：intent 截断 UTF-8 panic（B-02）

`src/intent/heuristic.rs:602` 本地 `truncate` 用 `&s[..max_len]` 字节切片，
任意用户文本在 UTF-8 边界上直接 panic（≥17 个汉字即超 50 字节）。改法：
5 个调用点（:289/:311/:319/:406/:450）全部改用仓内
`crate::util::safe_truncate`（`src/util.rs:47`），随后删除本地 `truncate`。
补一个多字节边界定向测试（含中文长文本走全部 5 个调用点路径，断言零 panic
且结果不超上限）。

### W-05：token 用量估算接入 provider usage 字段（B-03）

现状 chars/4：`src/llm/ollama.rs:95-96`（已核实），同法重复于
`src/kernel/ops/cost_ledger.rs:109`、`src/kernel/cognition/context_quality.rs:410-417`、
`benchmarks/**/metrics.py:143-150`。改法：Ollama 响应已解析
`prompt_eval_count`/`eval_count`——读真实 usage 字段；字段缺失（旧版 server）
时回退现有 chars/4 估算并注释说明。**禁止**引入 `tiktoken-rs` 或任何新依赖
（那是备选方案，需单独架构授权；W0 明确"不引入通用 tokenizer 冒充真实
usage"）。三处 Rust 站点 + benchmarks 站点各补定向测试/断言（benchmarks 走
其自身 pytest 门）。

### B-10：temporal 死代码与失实文档

删除死代码：`TimeRange::expanded`（`src/temporal/resolver.rs:26-36`，全仓零
调用点）、`Granularity::HalfYear`（`src/temporal/rules.rs:24`，仅自身 Debug
测试引用，连同该断言一起删）、`util::safe_range`（`src/util.rs:55`，零引用）。
±7 天置信扩展的失实文档按"删失实、不加新语义"处理（W0 不实现新 temporal
行为）；删除后全量 grep 确认零残留引用。

### B-11：benchmarks 死指标

`benchmarks/**/metrics.py` 中实际未被使用的 `ndcg_at_k`/`mrr`/`recall_at_k`
（真实评估走 ir-measures）——删除死指标函数与导入，跑 benchmarks 自身
pytest 门确认绿。

### B-12：INDEX 失实描述（恰 2 处，**不含 intent/INDEX.md**）

`src/temporal/INDEX.md`（描述不存在的 OllamaTemporalResolver 及 ±7 天扩展）
与 `src/mcp/INDEX.md:29`（称用 tokio 实为同步 std）——改为与代码一致。
intent/INDEX.md 明确排除在 W0 外（留给后续轮次）。

## 2. 允许修改（exact 清单）

```
src/intent/heuristic.rs            # W-06 调用点 + 删本地 truncate + 内联测试
src/util.rs                        # 仅删 safe_range（B-10）；不得动 safe_truncate 语义
src/llm/ollama.rs                  # W-05 + 内联测试
src/kernel/ops/cost_ledger.rs      # W-05 + 测试
src/kernel/cognition/context_quality.rs  # W-05 + 测试
src/temporal/resolver.rs           # B-10 删 expanded + 内联测试同步清理
src/temporal/rules.rs              # B-10 删 HalfYear + 测试断言同步清理
src/temporal/INDEX.md              # B-12
src/mcp/INDEX.md                   # B-12
benchmarks/**/metrics.py           # B-11 + W-05 第 4 站点（路径以实际为准，限 metrics 模块）
docs/milestones/v53-w0-*.md        # 开发组自己的交付说明（可选）
```

## 3. 禁止

- 上述清单之外的一切文件（尤其：`src/cas/**`、`src/memory/**`、
  `src/api/**`、`src/mcp/**`（除 INDEX.md）、`src/scheduler/**`、
  `src/bin/**`、`Cargo.toml`/`Cargo.lock`——**零新依赖**、AGENTS.md、
  `docs/adr/**`、intent/INDEX.md）。
- 不实现新 temporal 语义；不重构超出删除/替换一行的结构；不改公共 API 签名
  （`safe_truncate` 既有签名不得动）。
- 不顺手修任何其他 B-*/W-* 条目（D-MCP-2/3 归外包架构组；redb 仅 research）。

## 4. 成本控制与门（按序执行）

1. 静态 scope 自检：`git diff --name-only <基线>..HEAD` ⊆ 第 2 节清单；
   新增说明文档过共用 path-free validator（不写任何本机绝对路径）；
2. 定向测试：intent 边界、ollama usage、cost_ledger、context_quality、
   temporal 编译清理（`EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test
   --locked --offline --lib intent_ temporal_ ollama cost_ledger
   context_quality` 按需分跑）；
3. benchmarks 门（若触碰）：`cd benchmarks && uv sync --locked --offline
   --extra dev && uv run --offline pytest -q`；
4. `cargo fmt --all -- --check`、`git diff --check`、
   `cargo clippy --locked --offline --all-targets --all-features -- -D warnings`；
5. 以上全绿后，全库 `--lib --all-features` 恰跑一次（基线 2146/0/2，
   本机负载尖峰下 D1 家族 flake 为环境项，隔离复跑定性，不追改）。

构建环境沿仓规：repo 外 `CARGO_TARGET_DIR`、`CARGO_NET_OFFLINE=true`、stub
双后端（具体值见各runner本地门约定，文档内不落本机路径）。

## 5. 交付格式

- candidate SHA + `git diff --stat <基线>..HEAD`；
- 第 4 节各门的**原始摘要**（定向/全库/benchmarks）；
- 死代码删除的零残留 grep 证据（expanded/HalfYear/safe_range）；
- 已知限制与 Architecture Deviation（如有，越界前先提）。

## 6. 非目标

kernel/scheduler producer admission、public API/MCP/aicli/daemon 接线、
O(n²) append（P2，owner 外包架构组）、文件长度拆分（P2，后续轮）、
W-01..W-22 其余任何条目、fixture→trusted promotion。
