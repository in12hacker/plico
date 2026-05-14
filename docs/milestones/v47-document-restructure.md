# v47 里程碑：文档体系重构

**日期**：2026-05-14
**目标**：重构项目所有规则文档和记忆文件，建立严格的开发流程规范，消除文档混乱。
**范围**：仅文档重构，不涉及代码开发。

---

## 1. 背景与问题

### 1.1 核心问题

经过对 72 个文档文件的全面审计，发现以下系统性问题：

| 问题 | 严重度 | 具体表现 |
|------|--------|---------|
| CLAUDE.md 结构性混乱 | P0 | 589 行，两个文档硬拼，版本快照混入规则文件，中英文混用无章法 |
| 缺乏模块化规则加载 | P0 | 未使用 `.claude/rules/` 按需加载，所有规则塞入单文件 |
| AGENTS.md 定位模糊 | P1 | 混合目录地图、构建命令、Benchmark 文档，职责不清 |
| 无正式开发流程规范 | P0 | 缺少里程碑模板、测试门控标准、退化判定规则 |
| 违背 dogfood 原则 | P1 | MEMORY.md 存储了应迁移至 CAS 的历史进度 |
| 引用错误 | P1 | CLAUDE.md 引用不存在的 `system.md` |

### 1.2 用户决策记录

| 问题 | 决策 |
|------|------|
| Q1 通用行为准则 | 迁移到 `~/.claude/CLAUDE.md`（全局共享） |
| Q2 版本快照 | 迁移到 `docs/milestones/`，加引导规则防止再次污染 |
| Q3 Benchmark 文档 | 合并到 `benchmarks/README.md`，加软索引 |
| Q4 开发流程规范 | 写入 CLAUDE.md 作为永久规则 |
| Q5 语言 | 全部中文 |
| Q6 覆盖率门控 | 全局 ≥ 90%，不可覆盖模块用代码注释标注 |
| Q7 MEMORY.md 历史 | 迁移到 Plico CAS（dogfood） |
| Q8 settings.local.json | 保持现状 |

---

## 2. 重构方案

### 2.1 目标文件结构

```
Plico/
├── CLAUDE.md                          # 项目级永久规则（精简，中文，< 300 行）
│   ├── 项目概述（10 行）
│   ├── 架构要点（15 行）
│   ├── 构建与测试命令（30 行）
│   ├── 推断后端配置（15 行）
│   ├── 工具配置（MCP 等，15 行）
│   ├── 开发流程规范（永久规则，60 行）  ← 新增，核心
│   │   ├── 里程碑开发流程
│   │   ├── 测试门控标准
│   │   ├── 退化判定与修复循环
│   │   └── 端到端测试规范
│   ├── 安全红线（30 行）
│   ├── Tokio 运行时模式（15 行）
│   ├── 测试编写模式（20 行）
│   ├── AI 导航指引（10 行）
│   └── 版本快照存储规则（5 行）  ← 引导到 docs/milestones/
│
├── .claude/
│   ├── settings.json                  # 保持现状
│   ├── settings.local.json            # 保持现状
│   └── rules/                         # 按需加载规则（新增目录）
│       ├── coding-principles.md       # 从 CLAUDE.md 迁出的通用行为准则
│       ├── benchmark.md               # Benchmark 框架操作指南（从 AGENTS.md 迁出）
│       └── development-workflow.md    # 开发流程详细说明（CLAUDE.md 的展开版）
│
├── AGENTS.md                          # Agent 协作导航（精简，中文，< 200 行）
│   ├── 项目概述
│   ├── 目录地图（精简版）
│   ├── 快速导航表
│   ├── 代码规范
│   ├── 架构约束
│   ├── Agent 工作流检查清单
│   └── 环境变量表
│
├── docs/
│   ├── milestones/                    # 里程碑文档（新增目录）
│   │   ├── INDEX.md                   # 里程碑索引
│   │   ├── v34-summary.md             # 从 docs/ 迁移
│   │   ├── v35-summary.md             # 从 docs/ 迁移
│   │   ├── v41-summary.md             # 从 docs/plans/ 迁移
│   │   ├── v43-summary.md             # 从 docs/ 迁移
│   │   └── v46-summary.md             # 从 CLAUDE.md 迁出的版本快照
│   └── plans/                         # 保持现状
│
├── benchmarks/
│   └── README.md                      # 合并后的 Benchmark 完整文档
│
└── ~/.claude/CLAUDE.md                # 个人全局配置（新增）
    └── 通用行为准则（Think Before Coding 等）
```

### 2.2 CLAUDE.md 重构规则

**核心原则**："精准优于全面"（Litmus Test 编写法）

每条规则必须通过以下测试：
1. **是否会导致错误行为？** — 如果删除这条规则，AI 是否会犯错？如果不会，删除。
2. **是否可从代码推导？** — 如果可以通过读代码得知，不写入 CLAUDE.md。
3. **是否有时效性？** — 如果会过期，不写入 CLAUDE.md，引导到 `docs/milestones/`。

**禁止写入 CLAUDE.md 的内容**：
- 版本快照（测试数量、覆盖率数字、QPS 指标）→ `docs/milestones/vXX-summary.md`
- 通用编程准则（Think Before Coding 等）→ `~/.claude/CLAUDE.md`
- Benchmark 操作细节 → `.claude/rules/benchmark.md`
- 具体的开发教训总结（Phase 1-4 完成总结）→ `docs/milestones/vXX-summary.md`
- 可从代码推导的 API 细节 → 读源码

**必须写入 CLAUDE.md 的内容**：
- 项目概述（是什么、不是什么）
- 构建/测试/运行命令
- 安全红线（不可推导的硬约束）
- 开发流程规范（永久规则）
- 推断后端配置（环境变量依赖，不可从代码推导）
- 测试覆盖率门控标准

### 2.3 开发流程规范（写入 CLAUDE.md 的永久规则）

```
## 开发流程规范

### 里程碑开发流程

每个里程碑必须经历以下阶段，不可跳过：

1. **里程碑规划**
   - 创建 `docs/milestones/vXX-<name>.md`
   - 包含：目标、任务拆分、验收标准、风险评估
   - 每个任务必须有明确的"完成"定义

2. **模块开发**
   - 按功能模块逐个开发，不可一次性生成全部代码
   - 每个模块必须覆盖测试用例
   - 新增代码覆盖率 ≥ 90%（全局 `cargo llvm-cov --lib` ≥ 90%）
   - 不可覆盖的模块用 `// coverage:skip <reason>` 注释标注

3. **质量门控（每个模块完成后）**
   - `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib` 全通过
   - `cargo clippy -- -D warnings` 无新增警告
   - 无 O(n²) 算法（关键路径需性能测试）

4. **里程碑验收（所有模块完成后）**
   - 回归测试：`cargo test` 全量通过
   - 覆盖率：`cargo llvm-cov --lib` ≥ 90%
   - 性能回归：`cargo test --test perf_regression` 通过
   - 可达性测试：验证里程碑承诺的所有能力
   - 退化检测：对比里程碑前后的行为差异

5. **端到端测试**
   - 运行 benchmark suite 验证业务流程
   - 生成 benchmark 报告到 `benchmarks/results/`
   - 推演下一阶段里程碑

### 退化判定规则

以下任一条件成立即判定为退化，里程碑开发失败，必须重新进入开发阶段修正：

- `cargo test` 出现新增失败
- 覆盖率下降（低于里程碑前的基线）
- 性能回归测试失败（P50/P95 超过阈值）
- Clippy 出现新增警告
- Benchmark 指标下降（对比上一版本报告）

### 版本快照存储规则

版本特定的数据（测试数量、覆盖率、QPS、benchmark 结果等）**禁止写入 CLAUDE.md**。
存储位置：`docs/milestones/vXX-summary.md`

格式模板：
```markdown
# vXX 里程碑总结

## 质量基线
- 测试：N 个（lib + integration + perf）
- 覆盖率：XX.XX%
- Clippy：N 个新增警告
- 性能回归：N/N 通过

## Benchmark 结果
| 指标 | 值 |
|------|-----|
| ... | ... |

## 关键变更
- ...

## 遗留问题
- ...
```
```

### 2.4 `.claude/rules/` 文件内容

#### `rules/coding-principles.md`（从 CLAUDE.md 迁出）

内容：Think Before Coding、Simplicity First、Surgical Changes、Goal-Driven Execution、No Compatibility Code。共 5 条规则，保持英文原文（通用准则不限语言）。

#### `rules/benchmark.md`（从 AGENTS.md + CLAUDE.md 合并）

内容：
- Benchmark 框架目录结构
- 模型矩阵（llama.cpp 强制）
- 预处理阶段说明
- 数据格式陷阱
- 进程与脚本规范
- 环境变量表
- 多模型配置说明
- 软索引到 `benchmarks/README.md`

#### `rules/development-workflow.md`（CLAUDE.md 开发流程的展开版）

内容：
- 里程碑模板详细说明
- 测试门控标准详细说明
- 退化判定详细说明
- 端到端测试详细说明
- Benchmark 报告解读与推演方法

### 2.5 AGENTS.md 精简规则

删除以下内容（已迁移到其他位置）：
- Benchmark 框架文档（→ `.claude/rules/benchmark.md`）
- 重复的环境变量表（保留一份在 CLAUDE.md）

保留以下内容：
- 项目概述（中文）
- 目录地图（中文，精简版）
- 快速导航表
- 代码规范
- 架构约束
- Agent 工作流检查清单

---

## 3. 任务拆分

| 序号 | 任务 | 验证标准 |
|------|------|---------|
| T1 | 创建 `~/.claude/CLAUDE.md`，写入通用行为准则 | 文件存在，内容正确 |
| T2 | 创建 `.claude/rules/` 目录及 3 个规则文件 | 文件存在，内容正确 |
| T3 | 创建 `docs/milestones/v46-summary.md`，从 CLAUDE.md 迁出版本快照 | v46 数据从 CLAUDE.md 中删除 |
| T4 | 迁移已有里程碑文档到 `docs/milestones/` | 文件位置正确，旧文件删除 |
| T5 | 重构 CLAUDE.md（精简至 < 300 行，全中文） | 通过 Litmus Test，行数达标 |
| T6 | 重构 AGENTS.md（精简至 < 200 行，全中文） | 无 Benchmark 细节，行数达标 |
| T7 | 合并 Benchmark 文档到 `benchmarks/README.md` | 内容完整，软索引正确 |
| T8 | 迁移 MEMORY.md 历史到 Plico CAS | MEMORY.md 仅保留索引 |
| T9 | 创建 `docs/milestones/INDEX.md` | 索引完整 |
| T10 | 最终验证 | 所有文件存在，无断链，无重复内容 |

---

## 4. 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| `.claude/rules/` 不被所有工具识别 | 部分 AI 工具可能忽略 rules/ 目录 | 关键规则仍保留在 CLAUDE.md 中 |
| 过度精简导致信息丢失 | AI 缺少必要上下文 | 用软索引（See @file）引用详细文档 |
| 中文翻译导致术语不一致 | 与代码中的英文术语冲突 | 关键术语保留英文（如 CAS、HNSW、RRF） |
| dogfood 迁移命令失败 | 历史数据丢失 | 先备份 MEMORY.md，再执行迁移 |

---

## 5. 验收标准

- [ ] CLAUDE.md < 300 行，全中文，通过 Litmus Test
- [ ] AGENTS.md < 200 行，全中文，无 Benchmark 细节
- [ ] `.claude/rules/` 包含 3 个规则文件
- [ ] `docs/milestones/` 包含所有历史里程碑文档
- [ ] `benchmarks/README.md` 包含完整的 Benchmark 文档
- [ ] MEMORY.md 仅保留索引，无历史进度数据
- [ ] 无断链（所有引用的文件都存在）
- [ ] 无重复内容（同一信息只出现在一个位置）
- [ ] 开发流程规范已写入 CLAUDE.md 作为永久规则
- [ ] 版本快照存储规则已写入 CLAUDE.md
