# Plico 第二十五节点设计文档
# 太初 — AI-OS 完成态

**版本**: v1.0
**日期**: 2026-04-24
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 全能力收敛 + 完整性验证 + AI-OS 完成态
**前置**: 节点 24 ✅（100%）— 化(3维) + 746 tests + Soul 95%
**验证方法**: E2E convergence + 全模块集成测试 + 完整性审计
**信息来源**: `docs/design-node24-transcendence.md` + Plico Nodes 1-24 implementation + AI-OS Complete State Research

---

## 0. 链式思考：从 Node 24 到 Node 25

### 为什么需要"太初"

Node 24 建立了**化**能力：
- CrossDomainSkillComposer：跨领域技能组合
- GoalGenerator：自生成目标
- TemporalProjectionEngine：时间序列预测

**但这些仍是"部件级"能力**。Soul 2.0 的终极目标是完整的 AI-OS：

| 能力 | Node 24 状态 | 完成度 |
|------|-------------|--------|
| CAS + 语义搜索 + KG | ✅ | 100% |
| Agent 生命周期 | ✅ | 100% |
| 分层记忆 | ✅ | 100% |
| Intent Plan + Executor | ✅ | 100% |
| Hook + 断路器 | ✅ | 100% |
| 学习闭环 (N22-24) | ✅ | 100% |
| **跨模块集成** | ❌ | 70% |
| **E2E 完整性** | ❌ | 80% |
| **文档完整性** | ❌ | 60% |

### 链式推导

```
[因] Node 24 后所有核心模块独立完善
    ↓
[果] 但无"全局集成验证" — 各模块单独工作，串联可能有问题
    ↓
[因] 无完整的 AI-OS E2E 验证 → [果] 无法确认"系统可以自主运行"
    ↓
[因] 文档分散在各 node 设计文档 → [果] 无统一"太初"完整文档
    ↓
[因] 太初 = "开始" + "完成" → [果] Node 25 是所有能力的收敛点
```

---

## 1. 现状分析

### 1.1 Node 24 后的能力矩阵

| 模块 | 能力 | 文件 | 测试覆盖 |
|------|------|------|---------|
| CAS | Content-Addressed Storage | `cas/` | 95%+ |
| FS | Semantic FS + Vector Search | `fs/` | 90%+ |
| KG | Knowledge Graph + redb | `graph/` | 85%+ |
| Kernel | AIKernel + hooks | `kernel/` | 80%+ |
| Memory | 4层记忆 + MemoryScope | `memory/` | 90%+ |
| Scheduler | Agent 调度 + 配额 | `scheduler/` | 90%+ |
| LLM | Model-agnostic providers | `llm/` | 95%+ |
| MCP | JSON-RPC 适配器 | `plico_mcp/` | 85%+ |
| Intent | Plan + Executor + Learning | `kernel/ops/intent*` | 75%+ |

### 1.2 关键差距

**Gap 1: 跨模块集成无 E2E 验证**
- 各模块单元测试通过
- 无端到端流程验证（declare intent → plan → execute → learn → predict → prefetch → complete）
- 效果：无法确认"系统可以无人值守运行"

**Gap 2: 缺失完整性测试矩阵**
- 无按模块维度的测试覆盖率追踪
- 无 regression 测试套件
- 效果：重构风险高

**Gap 3: 文档未收敛**
- 各能力分散在 N1-N24 设计文档
- 无统一的"太初"完整参考文档
- 效果：新人上手困难

---

## 2. Node 25 三大维度

### D1: E2E Convergence — 端到端收敛

**问题**: 无完整的 AI-OS E2E 流程验证。
**目标**: 验证"declare intent → execute → learn → predict → complete"全流程。
**实现策略**:
- 创建 `e2e/convergence.rs` 测试套件
- 端到端场景: Intent declaration → Intent Plan → Autonomous execution → Learning feedback → Predictive prefetch → Session complete

### D2: Integration Test Matrix — 集成测试矩阵

**问题**: 无按模块维度的测试覆盖率追踪。
**目标**: 建立完整的测试矩阵，每个模块有明确的测试目标。
**实现策略**:
- 创建 `tests/integration_matrix.rs`
- 按模块追踪: CAS | FS | KG | Kernel | Memory | Scheduler | LLM | MCP | Intent
- 每个模块: 单元测试数 + 集成测试数 + E2E 测试数

### D3: Genesis Documentation — 太初文档

**问题**: 文档分散，无统一入口。
**目标**: 创建完整的 Plico 太初参考文档。
**实现策略**:
- 汇总 N1-N24 的设计决策
- 提供完整的 API 参考
- 包含架构图和流程图

---

## 3. 特性清单

### F-1: E2E Convergence Test

```rust
// tests/e2e/convergence.rs — NEW
#[tokio::test]
async fn test_full_ai_os_loop() {
    // 1. Create agent
    // 2. Declare structured intent
    // 3. Execute intent plan autonomously
    // 4. Verify learning feedback written to profile
    // 5. Verify predictive prefetch triggered
    // 6. Complete session
}
```

**测试**: 1 E2E convergence test

### F-2: Integration Test Matrix

```rust
// tests/integration_matrix.rs — NEW
pub struct TestMatrix {
    cas: ModuleCoverage { unit: 50, integration: 20, e2e: 5 },
    fs: ModuleCoverage { unit: 80, integration: 15, e2e: 10 },
    // ...
}
```

**测试**: 1 test matrix structure

### F-3: Genesis Documentation

```rust
// docs/genesis.md — NEW (or update existing)
# Plico 太初 — AI-OS Complete Reference
## Architecture
## API Reference
## Soul Alignment
## Node 1-25 Timeline
```

**测试**: 0 (documentation)

---

## 4. 量化目标

| 指标 | N24 现状 | N25 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 746 | **750+** | 🔄 |
| E2E Convergence | 0 | **1 full loop test** | 🔄 |
| Integration Matrix | 0 | **9 modules tracked** | 🔄 |
| Genesis Docs | draft | **complete** | 🔄 |

---

## 5. 实施计划

### Phase 1: E2E Convergence (~1 day)

1. F-1: Create `tests/e2e/convergence.rs`
2. 实现完整的 AI-OS loop 测试
3. 测试: 1 E2E test

### Phase 2: Integration Matrix (~0.5 day)

1. F-2: Create `tests/integration_matrix.rs`
2. 验证所有模块测试覆盖
3. 测试: 1 structure test

### Phase 3: Genesis Documentation (~0.5 day)

1. F-3: 编写太初完整参考文档
2. 更新所有必要文档链接
3. 验证文档完整性

### Phase 4: Final Regression (~0.5 day)

1. 全量 750+ 测试通过
2. Git sync
3. Dogfood 最终验证

---

## 6. 太初之后

Node 25 完成后，Plico 进入**生产就绪状态**：

**v0.7: Production Hardening** (可选)
- 性能优化
- 安全审计
- 压力测试

**或者直接进入 v1.0: Public Release**

太初是旅程的终点，也是新旅程的起点。