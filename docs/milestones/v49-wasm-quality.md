# v49 里程碑：WASM Runtime & Quality Hardening

**日期**：2026-05-14
**目标**：交付首个真实 WASM 技能执行运行时，修复 benchmark 和 DSL 积累的技术债，强化认知模块覆盖率
**范围**：`src/kernel/cognition/`（WASM、DSL、composer）、`benchmarks/`（质量修复）、`src/memory/`（因果图谱集成）、`src/kernel/ops/`（Agent 注册）。唯一新依赖：wasmtime

---

## 1. 背景与问题

### 1.1 核心问题

v48 完成了认知引擎 12 个 TODO，遗留 2 个 WASM 相关 TODO。v49 在此基础上解决 8 个问题：

| 问题 | 严重度 | 位置 | 影响 |
|------|--------|------|------|
| WASM 运行时是空壳 | P0 | `wasm_runtime.rs` | Code 技能无法执行——SkillForge 的 Code 路径死路 |
| DSL ToolCall 返回 "Unknown tool" | P0 | `dsl_interpreter.rs:92` | 使用 ToolCall 步骤的 Config 技能运行时必失败 |
| SkillComposer 仅处理 Knowledge | P1 | `skill_composer.rs:30` | Config 和 Code 技能组合时被静默跳过 |
| Benchmark 版本号硬编码为 "v44" | P1 | `benchmarks/` 15+ 处 | 报告显示错误版本；无 --version 参数 |
| Benchmark 确定性采样 | P1 | `conversational_qa.py` | 始终取前 N 条；无随机化，结果有偏 |
| CausalGraph 从未在搜索路径中使用 | P1 | `src/kernel/ops/memory.rs:699` | `causal_graph: None`——因果信号未参与检索融合 |
| Agent 懒注册不完整 | P2 | `src/kernel/ops/agent.rs:29` | `ensure_agent_registered` 跳过 KG 锚点、事件、持久化 |
| 认知模块覆盖率缺口 | P2 | 多个文件 77-82% | context_quality、skill_forge、skill_registry、skill_validator 未测试路径 |

### 1.2 与 v48 的衔接

| 维度 | v48 状态 | v49 目标 |
|------|---------|---------|
| 技能类型 | Knowledge + Config 端到端可用 | Knowledge + Config + Code 全部可用 |
| DSL 解释器 | ToolCall / If / ForEach / Parallel / Recall / Store 全部可用 | ToolCall 连接真实工具执行 |
| 技能组合 | 仅 Knowledge 类型 | Knowledge + Config + Code + 混合类型 |
| 因果图谱 | CausalHook 写入 KG，但搜索路径未使用 | CausalGraph 在检索融合中生效 |
| Benchmark | 版本硬编码 v44，确定性采样 | 版本参数化，随机采样，CoT 提取鲁棒 |
| 覆盖率 | 87.17%（4 个文件 77-82%） | ≥ 88%（所有目标文件） |

### 1.3 决策记录

| 问题 | 决策 |
|------|------|
| WASM 运行时选择 | wasmtime（设计文档 `soul-v3-architecture.md:974` 指定；fuel 内置） |
| wasmtime 是否可选 | 是。`cfg(feature = "wasmtime-backend")` 避免增加所有人编译时间 |
| DSL 工具调用方案 | ToolExecutor trait（避免 DslInterpreter ↔ AIKernel 循环依赖） |
| F1 指标是否替换 | 否。保留 token-level F1，在报告中说明局限性 |
| CausalGraph 构建策略 | 惰性按查询构建（仅当条目有 causal_parent/supersedes 字段时） |

---

## 2. 方案设计

### 2.1 WASM 运行时架构

```
SkillForge.execute_code_skill()
    ↓
WasmRuntime.execute(wasm_bytes, inputs, limits)
    ├── 1. 模块编译：检查缓存 (RwLock<HashMap<sha256, Module>>)
    │       cache miss → wasmtime::Module::new(&engine, wasm_bytes)
    ├── 2. Store 创建：fuel = limits.max_execution_time_ms * 1000
    ├── 3. Host 函数注入：plico_log, plico_tool_call
    ├── 4. 执行：instance.get_func("main")?.call()
    │       fuel 耗尽 → WasmExecutionFailed
    └── 5. 捕获输出：读取返回值或共享内存缓冲区
```

### 2.2 DSL ToolCall 架构

```
DslInterpreter { executor: Option<Arc<dyn ToolExecutor>> }

trait ToolExecutor: Send + Sync {
    fn execute_tool(&self, name: &str, params: &Value, agent_id: &str) -> ToolResult;
}

AIKernel 实现 ToolExecutor（委托 self.execute_tool）

execute_step(ToolCall) → resolve_params → executor.execute_tool → context.set_variable
```

### 2.3 SkillComposer 扩展

```
compose(skills) → 匹配技能类型：
    全 Knowledge → 现有行为（合并知识条目）
    全 Config    → 顺序合并 tool_chains，合并 parameter_mappings
    混合类型     → Knowledge 提供上下文文档，Config/Code 提供执行步骤
                   生成 ConfigSkill，知识条目注入为 DSL 变量
```

### 2.4 CausalGraph 集成

```
memory_retrieve(query, agent_id, config)
    ├── entries = layered_memory.recall(...)
    ├── if 任何条目有 causal_parent 或 supersedes：
    │       causal_graph = CausalGraph::build(&entries)
    └── rfe.rank(&entries, &query, causal_graph, top_k)
```

### 2.5 核心规则

- wasmtime 作为 optional feature，默认不启用，CI 需测试 with/without 两条路径
- ToolExecutor trait 定义在 `dsl_interpreter.rs`，AIKernel 在 `mod.rs` 实现
- Benchmark --version 从环境变量或 CLI 参数传播，不再硬编码
- CausalGraph 仅在条目有因果字段时构建，避免无谓开销

---

## 3. 任务拆分

| 序号 | 任务 | 文件 | 验证标准 | 状态 |
|------|------|------|---------|------|
| T1 | Benchmark --version 参数 | `run_full_benchmark.sh`, `cli.py`, `harness.py`, 6 个 suite 文件 | `grep -r "v44" benchmarks/` 返回 0 结果；`--version v49` 输出正确版本 | ✅ |
| T2 | Benchmark 随机采样 | `conversational_qa.py` | `--seed 42` 产生确定性但随机化的样本选择 | ✅ |
| T3 | Benchmark CoT 提取鲁棒性 | `conversational_qa.py` | `_extract_answer` 处理 "Answer:" 标记、纯文本、JSON、空响应 | ✅ |
| T4 | DSL ToolCall 修复 | `dsl_interpreter.rs`（新 trait + 字段）、`mod.rs`（AIKernel impl） | DslInterpreter 带 executor 可调用真实工具；ToolCall 步骤存储结果到 context | ✅ |
| T5 | WASM 运行时实现 | `Cargo.toml`（新依赖）、`wasm_runtime.rs`（完整重写） | `WasmRuntime::new()` 成功；`execute()` 编译、缓存并运行最小 WASM 模块；fuel 耗尽返回错误 | ✅ |
| T6 | SkillValidator WASM 验证 | `skill_validator.rs:218` | `validate_code_skill` 编译 WASM 字节码，报告编译错误为验证问题 | ✅ |
| T7 | SkillComposer Config 组合 | `skill_composer.rs` | `compose()` 处理 Config 技能，产生合并的 ConfigSkill（tool_chains 顺序拼接） | ✅ |
| T8 | SkillComposer 混合类型组合 | `skill_composer.rs` | `compose()` 处理 Knowledge + Config 混合，Knowledge 条目作为 DSL 变量注入 | ✅ |
| T9 | CausalGraph 搜索路径集成 | `src/kernel/ops/memory.rs:699` | 搜索含因果条目时构建 CausalGraph 并传给 RFE；根因查询端到端可用 | ✅ |
| T10 | Agent StartSession 自动注册 | `src/kernel/ops/session.rs:567+`、`tests/ai_experience_test.rs:257` | StartSession 对未注册 Agent 自动创建 KG 锚点 + 发出 AgentStateChanged 事件 | ✅ |
| T11 | 覆盖率：context_quality.rs | `context_quality.rs` | `find_superseder`（KG）、`identify_removable`（KG）、`generate_summaries`、compress "retained < half" 均有测试 | ✅ |
| T12 | 覆盖率：skill_forge.rs | `skill_forge.rs` | `execute_config_skill` 真实路径、`execute_code_skill` 真实路径、Config/Code 相关性计算均有测试 | ✅ |
| T13 | 覆盖率：skill_registry.rs | `skill_registry.rs` | `with_persistence`、`persist`、`restore`、`get_record` 均有测试 | ✅ |
| T14 | 覆盖率：skill_validator.rs | `skill_validator.rs` | `validate_config_skill`、`validate_code_skill`、Checklist/Lesson/Warning 知识类型、Config/Code 冲突检测均有测试 | ✅ |
| T15 | WASM 性能回归测试 | `tests/perf_regression.rs` | `perf_wasm_compile` P50 < 50ms；`perf_wasm_execute` P50 < 1ms | ✅ |
| T16 | 质量门控 | — | 全量测试通过 + 覆盖率 ≥ 87.17% + Clippy 零新增 + 性能回归通过 | ✅ |

**任务依赖**：
```
T1+T2+T3（benchmark 修复）── 独立，优先执行
T4（DSL ToolCall）── 独立，T7/T8 的前置
T5+T6（WASM 运行时）── 独立，大任务
T7+T8（SkillComposer）── 依赖 T4 + T5
T9（CausalGraph）── 独立
T10（Agent 自动注册）── 独立
T11-T14（覆盖率）── 依赖 T4-T10 完成后
T15（WASM 性能测试）── 依赖 T5
T16（质量门控）── 依赖全部
```

---

## 4. 质量门控

### 门控标准（每个模块完成后）

```bash
# 1. 全量 lib 测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 2. 覆盖率 ≥ 87.17%（v48 基线）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# 3. Clippy 无新增警告
cargo clippy -- -D warnings

# 4. 性能回归测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression

# 5. WASM 特定门控（T5、T6、T15 完成后）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib --features wasmtime-backend
cargo clippy --features wasmtime-backend -- -D warnings
```

### 退化判定规则

以下任一条件成立即判定为退化：

- `cargo test` 出现新增失败
- 覆盖率低于 87.17%（v48 基线）
- 性能回归测试失败（P50/P95 超过阈值）
- Clippy 出现新增警告
- Benchmark 指标下降（对比 v48 报告）

---

## 5. 风险评估

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| wasmtime 编译时间增加 | 高 | 中 | Feature-gate `wasmtime-backend`；CI 测试 with/without 两条路径 |
| WASM Host 函数注入安全 | 中 | 高 | 白名单允许的 host 函数；WASM 不能直接访问文件系统或网络 |
| DSL ToolCall 循环依赖 | 低 | 高 | ToolExecutor trait 打破循环；DslInterpreter 依赖 trait，AIKernel 实现 trait |
| CausalGraph 大结果集构建开销 | 低 | 中 | 仅当存在因果字段时构建；限制 top-K 条目（如 500） |
| 覆盖率测试破坏现有行为 | 中 | 中 | T11-T14 每个文件隔离测试；每完成一个运行全量测试 |
| Benchmark 采样变更破坏可复现性 | 低 | 低 | 默认 seed=42 确保确定性结果；在报告中记录 |

---

## 6. 验收标准

- [x] T1：benchmark 无硬编码 "v44"；`--version` 参数可用
- [x] T2：`--seed` 参数产生确定性随机化样本
- [x] T3：`_extract_answer` 处理所有 CoT 响应格式
- [x] T4：DSL ToolCall 通过 ToolExecutor trait 执行真实工具
- [x] T5：WASM Runtime 编译并执行模块，支持 fuel/内存限制
- [x] T6：SkillValidator 验证 WASM 字节码
- [x] T7：SkillComposer 组合 Config 技能
- [x] T8：SkillComposer 组合混合类型技能
- [x] T9：CausalGraph 在搜索路径中构建并使用
- [x] T10：StartSession 自动注册 Agent（KG 锚点 + 事件）
- [x] T11-T14：四个目标文件覆盖率 ≥ 88%
- [x] T15：WASM 性能回归测试在阈值内通过
- [x] T16：完整质量门控通过

---

## 7. 版本快照

### 质量基线
- 测试：2130 lib（with wasmtime-backend）/ 2124 lib（without） + 14 perf regression（12 base + 2 WASM）
- 覆盖率：87.72%（`cargo llvm-cov --lib`，v48 基线 87.17%，+0.55%）
- Clippy：0 新增警告
- 性能回归：14/14 通过（with wasmtime-backend）

### 覆盖率详情（目标文件）

| 文件 | 行覆盖率 | 分支覆盖率 |
|------|---------|-----------|
| context_quality.rs | 95.54% | 94.90% |
| skill_forge.rs | 91.15% | 85.71% |
| skill_registry.rs | 96.07% | 96.67% |
| skill_validator.rs | 97.25% | 97.56% |

### Benchmark 结果（v49，Qwen3 embedding）

| Suite | 指标 | 值 |
|-------|------|-----|
| performance | cas_write QPS | 24.1 |
| performance | search QPS | 4989.9 |
| performance | memory_recall QPS | 2291.5 |
| memory-crud | create success_rate | 100% |
| memory-crud | search hit_rate | 90% |
| memory-crud | batch_create avg_latency | 1974ms |
| conversational-qa | F1 | 0.212 |
| conversational-qa | LLM Score | 2.300 |
| conversational-qa | context_hit_rate | 100% |
| temporal-reasoning | F1 | 0.069 |
| temporal-reasoning | LLM Score | 0.867 |
| kg-reasoning | avg_latency | 0.212ms |

### 关键变更
- **Benchmark 框架**：版本号参数化（`PLICO_BENCH_VERSION` env / `--version` flag），消除 16 处硬编码（"v44" + "v46"）；随机采样（`PLICO_SEED` env）；CoT 提取支持 JSON / "Final Answer:" / "A:" 格式
- **DSL 解释器**：新增 `ToolExecutor` trait 打破循环依赖；`DslInterpreter` 支持可选 executor；ToolCall 步骤执行真实工具并存储结果
- **WASM 运行时**：完整 wasmtime 实现（feature-gated `wasmtime-backend`）；模块编译缓存（SHA-256）；fuel-based 执行限制；host 函数注入（plico_log、plico_tool_call 完整桥接）；内存限制；`compile_only` 方法用于验证
- **SkillValidator**：WASM 字节码编译验证（`compile_only`）；Config/Code 技能验证；所有 KnowledgeItem 类型覆盖
- **SkillComposer**：Config 技能组合（tool_chains 顺序拼接）；混合类型组合（Knowledge → DSL Store 变量注入）
- **CausalGraph**：搜索路径集成——含因果字段的条目自动构建 CausalGraph 并传给 RFE 排序
- **Agent 注册**：`ensure_agent_registered` 从懒注册升级为完整注册（KG 锚点 + 事件 + 持久化）
- **覆盖率强化**：4 个目标文件新增 40+ 测试，全部 ≥ 91%，使用真实依赖（KG、TempDir、WASM 编译）
- **WASM 性能回归**：`perf_wasm_compile` 和 `perf_wasm_execute` 添加到 `tests/perf_regression.rs`

### 遗留问题
- 无。v49 所有 16 个任务已完成。
