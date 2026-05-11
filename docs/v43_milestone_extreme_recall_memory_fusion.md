# Plico v43 进化里程碑：极致召回与记忆融合

## 0. 审计基线 (Audit Baseline)

### v42 交付状态审计

| 承诺项 | 状态 | 说明 |
| :--- | :--- | :--- |
| 二值量化检索 | ✅ 已交付 | 两阶段 Hamming+cosine，4 个测试通过 |
| 主动实体链接 | ✅ 已交付 | 3 级管线，集成到 kg_builder |
| 时序图整合 | ✅ 已交付 | temporal_diff + consolidate_versions |
| 认知冲突检测 | ⚠️ 已实现未接入 | ConflictDetector 代码存在但从未在生产中实例化 |
| Memory Passport | ❌ 未交付 | v42 报告承诺但未实现 |
| SimilarTo 边回归 | 🔧 已修复 | create() 缺少 upsert_document 调用，v43 修复 |

### v42 实测基准数据（已校正）

| 指标 | v42 实测值 | v42 报告原值 | 校正 |
| :--- | :--- | :--- | :--- |
| CAS Write QPS | 213 | 213 | ✅ |
| CAS Read QPS | 2,946 | 2,946 | ✅ |
| LoCoMo F1 (overall) | 15.3% | 15.6% | ≈持平 |
| LongMemEval EM | 27.4% | 27.4% | ✅ |
| MAB AR Hit Rate | **1.5%** | ~~68%~~ | ❌ 报告数据错误，已校正 |
| HotPotQA EM | 38.0% | 38.0% | ✅ |

---

## 1. 差距分析 (Gap Analysis)

### 1.1 检索质量差距

| 系统 | LoCoMo F1 | LongMemEval EM | 差距根因 |
| :--- | :--- | :--- | :--- |
| **Mem0 v3** (2026-04) | **91.6%** | **93.4%** | 单次检索 + 确认动作即存储 |
| **Tencent Agent Memory** | N/A | 76.1% (PersonaMem) | L0→L1→L2→L3 四层提炼 |
| **Plico v42** | 15.3% | 27.4% | 通用 prompt，无记忆融合 |

**根因分析**：
1. **Reader Prompt 策略**：Plico 使用通用 prompt，Mem0 针对 single-hop/multi-hop/temporal 使用专用 prompt
2. **记忆融合缺失**：Mem0 的"确认动作即存储"范式让每条信息以同等权重存储，Plico 的分层记忆导致信息丢失
3. **实体链接成熟度**：Mem0 的实体链接经过大规模生产验证，Plico 的 v42 管线是首次实现

### 1.2 性能差距

| 系统 | 检索延迟 (P50) | 写入 QPS | 定位 |
| :--- | :--- | :--- | :--- |
| **Synrix** | 0.028ms | 850k | 专用向量存储（晶格） |
| **Memvid v2** | 0.025ms | 1.3M | 视频编码向量存储 |
| **Plico v42** | 0.2ms | 213 | AI-OS 内核（CAS+语义+KG+认知） |
| **Mem0** | ~10ms | ~50 | 云端记忆服务 |

> Plico 的性能定位合理——AI-OS 内核 vs 专用向量存储，架构目标不同。

### 1.3 能力差距

| 能力 | Plico v42 | Mem0 v3 | Tencent | 差距 |
| :--- | :--- | :--- | :--- | :--- |
| 记忆融合 | ❌ | ✅ 顶级 | ✅ L0-L3 | 核心差距 |
| 冲突自修复 | ⚠️ 仅检测 | ❌ | ❌ | Plico 领先（需接入） |
| 记忆通行证 | ❌ | ❌ | ❌ | Plico 独有（需实现） |
| 多模态 | ❌ | ✅ 图片 | ❌ | 差距 |

---

## 2. v43 核心目标

**一句话**：从"认知自校准"进化到"极致召回与记忆融合"，在保持 Plico Soul（存储即认知）的前提下，将 LoCoMo F1 从 15% 提升至 50%+。

### 2.1 设计哲学（Plico Soul 对齐）

> **我们学习但不模仿。我们有我们自己的项目灵魂。**

- **不成为算法堆叠**：不追求 benchmark 数字的极致，而是构建"存储即认知"的完整闭环
- **记忆即认知**：每条记忆在存储瞬间即具备认知属性（语义索引 + KG 关联 + 冲突检测）
- **Agent 主权**：Agent 拥有自己的记忆，可以迁移（Passport）、自校准（Conflict）、自修复（Repair）
- **内核级融合**：记忆不是外部服务，而是 AI-OS 内核的核心能力

### 2.2 量化目标

| 目标 | v42 基线 | v43 目标 | 验证方式 |
| :--- | :--- | :--- | :--- |
| LoCoMo F1 | 15.3% | **≥50%** | LoCoMo benchmark |
| LongMemEval EM | 27.4% | **≥50%** | LongMemEval benchmark |
| MAB AR Hit Rate | 1.5% | **≥30%** | MemoryAgentBench |
| 冲突自修复 | 仅检测 | **自动修复** | 冲突→修复端到端测试 |
| Memory Passport | 无 | **可导出/导入** | 跨实例迁移测试 |
| 测试回归 | 2 个预存失败 | **0 个新失败** | cargo test |

---

## 3. 里程碑分解 (Milestone Breakdown)

### M1: Reader Prompt 优化 (Sprint 1)

**目标**：通过专用 prompt 策略提升检索质量，不改架构。

**行动**：
- 实现 `PromptStrategy` trait，支持 single-hop / multi-hop / temporal 三类 prompt
- 在 `search_with_filter` 中根据查询特征自动选择 prompt 策略
- 优化上下文组装：top-K snippets + KG neighbors + temporal context

**验证**：LoCoMo multi-hop F1 从 4.1% 提升至 ≥15%。

**风险**：低。纯 prompt 优化，不改架构。

---

### M2: 记忆融合引擎 (Sprint 2)

**目标**：实现"确认动作即存储"范式，追赶 Mem0 v3。

**行动**：
- 实现 `MemoryFusion` 模块：
  - Agent 确认的每个动作以同等权重存入记忆（不区分"重要/不重要"）
  - 自动去重：相同语义的记忆合并（利用 v42 实体链接）
  - 自动衰减：近期记忆权重更高（时间衰减函数）
- 在 `create()` 流程中集成融合逻辑
- 新增 `recall_with_fusion()` API：检索时考虑融合权重

**验证**：LoCoMo F1 提升至 ≥30%。

**风险**：中。需要调整记忆存储模型。

**Plico Soul 对齐**：记忆融合不是"算法堆叠"，而是让存储层更智能——每条记忆在存储时即完成融合决策，体现"存储即认知"。

---

### M3: 冲突自修复 (Sprint 3)

**目标**：从"检测"进化到"自动修复"。

**行动**：
- 将 `ConflictDetector` 接入 `CognitiveLoop`（当前是死代码）
- 实现 `ConflictResolver`：
  - `severity == High` 时自动 invalidate 置信度较低的边
  - `severity == Medium` 时发布 `ControlAction::Clarify` 意图
  - `severity == Low` 时记录到 DiagnosticStore，不自动处理
- 新增 `test_conflict_auto_repair` 端到端测试

**验证**：创建矛盾 KG 边 → 自动检测 → 自动修复 → 验证修复结果。

**风险**：中。自动修复可能误删有效边，需要置信度阈值调优。

---

### M4: Memory Passport (Sprint 4)

**目标**：支持 Agent 知识在不同内核实例间迁移。

**行动**：
- 定义加密记忆导出格式：`{version, agent_id, memories: [...], kg_edges: [...], signature}`
- 实现 `export_memories(agent_id, passphrase) -> Vec<u8>`
- 实现 `import_memories(data: &[u8], passphrase) -> Result<ImportReport>`
- 支持增量同步：基于 `DeltaSince` 序列号
- CLI 命令：`memory export --agent <id> --out <file>` / `memory import --file <path>`

**验证**：导出 → 新实例导入 → 验证记忆完整性。

**风险**：低。纯新增功能，不影响现有流程。

---

### M5: 端到端评测与回归验证 (Sprint 5)

**目标**：验证所有 v43 目标达成。

**行动**：
- 运行完整 benchmark 矩阵：LoCoMo / LongMemEval / MAB / HotPotQA / BEIR
- 运行 `cargo test` 全量测试，确保 0 新失败
- 运行 `cargo clippy`，确保 0 警告
- 更新 benchmark 报告

**验证**：所有量化目标达成。

---

## 4. 竞品技术分析（学习但不模仿）

### 4.1 Tencent Cloud Agent Memory (2026-04)

**架构**：L0(原始对话) → L1(原子事实) → L2(场景块) → L3(用户画像)

**可学习**：
- 四层提炼思路有价值——从原始数据到高层知识的渐进抽象
- PersonaMem 评测方法论：47.85% → 76.10% 的提升路径

**不模仿**：
- Plico 的 KG 已经实现了类似 L2/L3 的知识组织，不需要重复建设
- Plico 的"存储即认知"范式比"记忆即服务"更彻底——知识在存储瞬间即完成语义索引和 KG 关联

### 4.2 Mem0 v3 (2026-04)

**核心创新**：单次检索 + 确认动作即存储

**可学习**：
- "确认动作即存储"范式：Agent 确认的每条信息同等权重存储，不预判重要性
- Reader Prompt 策略：针对不同查询类型使用专用 prompt

**不模仿**：
- Mem0 是云端服务，Plico 是 AI-OS 内核——架构层级不同
- Mem0 没有 KG 和冲突检测，Plico 在知识管理维度更完整

### 4.3 AgeMem (Alibaba/Wuhan, 2026)

**核心创新**：统一 LTM/STM + 步进式 GRPO 强化学习

**可学习**：
- 统一记忆模型思路——不区分 STM/LTM，而是根据使用频率和时间自适应
- GRPO 强化学习用于记忆策略优化

**不模仿**：
- Plico 的分层记忆（Ephemeral → Working → Long-term → Procedural）是 Soul 3.0 的核心设计，不应放弃
- 可以在分层基础上引入衰减机制，但不改变分层架构

### 4.4 Tencent Agent Memory Pro (2026-04)

**核心创新**：基于向量数据库的企业级记忆服务，长任务 Token 消耗降 60%

**可学习**：
- 向量数据库作为记忆底层的思路
- 企业级场景的召回稳定性优化

**不模仿**：
- Plico 的 CAS + HNSW 已经是向量存储，不需要额外引入向量数据库

---

## 5. 技术债务清理

| 债务 | 优先级 | 行动 |
| :--- | :--- | :--- |
| ConflictDetector 死代码 | P0 | M3 中接入生产 |
| 2 个预存测试失败 | P1 | 排查 BM25 stub embedding 兼容性 |
| `start_workers()` 分散调用 | P2 | 统一到 `AIKernel::start()` 方法 |
| 未使用的 `tool_registry` | P3 | 清理或接入 |

---

## 6. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
| :--- | :--- | :--- | :--- |
| 记忆融合引入新 bug | 中 | 高 | 充分测试，灰度发布 |
| 冲突自修复误删有效边 | 中 | 高 | 置信度阈值调优，保留回滚能力 |
| LoCoMo 目标过高 | 中 | 中 | 分阶段目标：30% → 50% |
| 多模态延期 | 低 | 低 | v44 再做 |

---

## 7. 交付时间线

| Sprint | 里程碑 | 预计时间 | 依赖 |
| :--- | :--- | :--- | :--- |
| Sprint 1 | M1: Reader Prompt 优化 | 1 周 | 无 |
| Sprint 2 | M2: 记忆融合引擎 | 2 周 | M1 |
| Sprint 3 | M3: 冲突自修复 | 1 周 | 无 |
| Sprint 4 | M4: Memory Passport | 1 周 | 无 |
| Sprint 5 | M5: 端到端评测 | 1 周 | M1-M4 |

**总计**：约 6 周。

---

## 8. 验收标准 (Acceptance Criteria)

- [ ] LoCoMo F1 ≥ 50%
- [ ] LongMemEval EM ≥ 50%
- [ ] MAB AR Hit Rate ≥ 30%
- [ ] 冲突自修复端到端测试通过
- [ ] Memory Passport 导出/导入测试通过
- [ ] `cargo test` 全量通过（0 新失败）
- [ ] `cargo clippy` 0 警告
- [ ] v43 benchmark 报告发布

---

**里程碑负责人**：Claude Code (Autonomous Engineering Agent)
**创建日期**：2026-05-10
**状态**：v43 规划完成，待 Sprint 1 启动
