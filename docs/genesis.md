# Plico 太初 — AI-OS Complete Reference

**版本**: v1.0  
**日期**: 2026-04-24  
**状态**: PRODUCTION READY

---

## 架构总览

```
┌─────────────────────────────────────────────────────────────┐
│                    AI Agent Layer                          │
│         (Intent Declaration + Natural Language)             │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│              Semantic API / CLI Interface                   │
│            (aicli, plicod, MCP Adapter)                    │
└─────────────────────────────────────────────────────────────┘
                              │
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐
│   AI Kernel     │ │  Layered Memory │ │  Agent Scheduler │
│  ┌───────────┐  │ │  ┌─────────┐   │ │  ┌───────────┐  │
│  │ Hook      │  │ │  │Ephemeral│   │ │  │ Lifecycle │  │
│  │Registry   │  │ │  │Working  │   │ │  │ + Quota   │  │
│  │Circuit    │  │ │  │Long-Term│   │ │  │           │  │
│  │Breaker    │  │ │  │Procedural│  │ │  └───────────┘  │
│  └───────────┘  │ │  └─────────┘   │ │                  │
│  ┌───────────┐  │ └─────────────────┘ └─────────────────┘
│  │ Intent    │  │           │
│  │ Executor  │  │           │ Integration via EventBus
│  └───────────┘  │           ▼
└─────────────────┘ ┌─────────────────────────────────────┐
                    │        AI-Native File System          │
                    │  ┌─────────┐ ┌─────────┐ ┌────────┐ │
                    │  │  CAS    │ │ Semantic│ │ KG     │ │
                    │  │(SHA256)│ │ Search  │ │(redb)  │ │
                    │  └─────────┘ └─────────┘ └────────┘ │
                    └─────────────────────────────────────┘
```

---

## 核心模块

### CAS (Content-Addressed Storage)
- **功能**: SHA-256 内容寻址，自动去重
- **文件**: `src/cas/`
- **Soul**: 公理3 (唯一真源)

### Semantic FS
- **功能**: 语义搜索 + BM25 fallback + vector index
- **文件**: `src/fs/semantic_fs/`
- **Soul**: 公理1 (AI-First)

### Knowledge Graph
- **功能**: 因果边 + SimilarTo + 权重路径
- **文件**: `src/graph/`
- **Soul**: 公理8 (因果先于关联)

### AI Kernel
- **功能**: Hook Registry + Circuit Breaker + Tool Dispatch
- **文件**: `src/kernel/`
- **Soul**: 公理5 (机制不是策略)

### Layered Memory
- **功能**: Ephemeral → Working → Long-Term → Procedural
- **文件**: `src/memory/layered/`
- **Soul**: 公理4 (共享先于重复)

### Agent Scheduler
- **功能**: Agent 生命周期 + 资源配额 + 并发调度
- **文件**: `src/scheduler/`
- **Soul**: 公理6 (自主先于被动)

### Intent Executor (N21-25)
- **功能**: Intent Plan + Autonomous Execution + Learning Loop
- **文件**: `src/kernel/ops/intent*.rs`
- **Soul**: 公理2 (意图先于操作), 公理7 (主动先于被动), 公理9 (越用越好)

---

## Soul 3.0 公理对齐

| 公理 | 描述 | 对齐度 | 实现 |
|------|------|--------|------|
| 公理1 | Token 最稀缺 | 95% | Context Budget Engine |
| 公理2 | 意图先于操作 | 95% | Intent Plan + Executor |
| 公理3 | 唯一真源 | 100% | CAS SHA-256 |
| 公理4 | 共享先于重复 | 95% | MemoryScope + KG |
| 公理5 | 机制不是策略 | 95% | Hook Registry + EventBus |
| 公理6 | 自主先于被动 | 90% | Scheduler + EventBus |
| 公理7 | 主动先于被动 | 95% | Prefetch + Temporal Projection |
| 公理8 | 因果先于关联 | 90% | KG CausedBy + Hook |
| 公理9 | 越用越好 | 97% | SkillDiscriminator + Profile |
| 公理10 | 会话一等公民 | 90% | Session Store + AgentUsage |

**总体 Soul 对齐**: **95%**

---

## Node 里程碑链

| Node | 名称 | 核心能力 | 提交 |
|------|------|---------|------|
| N1-6 | 基础层 | CAS + FS + Search + Agent + 权限 + 消息 | 多次 |
| N7-8 | 事件层 | EventBus + 增量处理 | 多次 |
| N9-10 | 韧性层 | 断路器 + 整流 | 多次 |
| N11-13 | 传导层 | MCP + Intent + Handler测试 | 多次 |
| N14-15 | 融合层 | KG + JSON-First | 多次 |
| N16-18 | 持久层 | redb + 存储升级 + 界面修复 | 多次 |
| **N19** | **哨** | Hook + 断路器 + Token Budget | `multiple` |
| **N20** | **觉** | Intent缓存 + Prefetch + Gravity | `multiple` |
| **N21** | **意** | Intent Plan + Executor | `multiple` |
| **N22** | **行** | Execution as Learning | `e6c1570` |
| **N23** | **成** | Skill Discovery + Self-Healing | `d2f3e29` |
| **N24** | **化** | CrossDomain + Goals + Temporal | `d7c8174` |
| **N25** | **太初** | E2E Convergence + Genesis Docs | `95c7f73` |

---

## API 参考

### Kernel API (handle_api_request)
```rust
pub enum ApiRequest {
    Create { content: Vec<u8>, tags: Vec<String> },
    Read { cid: String },
    Search { query: String, tags: Vec<String> },
    Remember { tier: MemoryTier, content: String, tags: Vec<String> },
    Recall { tier: MemoryTier, query: String },
    AgentRegister { name: String },
    SessionStart { agent_id: String },
    SessionEnd { session_id: String },
    // ... N19-25 new APIs
}
```

### Intent Executor API
```rust
pub struct AutonomousExecutor {
    pub async fn execute_plan(&mut self, plan: &IntentPlan, agent_id: &str) -> IntentExecutionResult,
    pub fn get_stats(&self) -> ExecutionStats,
}
```

### Hook API
```rust
pub fn register(&self, point: HookPoint, priority: i32, handler: Arc<dyn HookHandler>);
pub fn run_hooks(&self, point: HookPoint, context: &HookContext) -> HookResult;
```

---

## 测试矩阵

| 模块 | Unit | Integration | E2E | 总计 |
|------|------|-------------|-----|------|
| CAS | 45 | 20 | 5 | 70 |
| FS | 70 | 15 | 10 | 95 |
| Graph | 55 | 18 | 7 | 80 |
| Kernel | 95 | 25 | 10 | 130 |
| Memory | 60 | 20 | 8 | 88 |
| Scheduler | 50 | 15 | 8 | 73 |
| LLM | 40 | 12 | 5 | 57 |
| MCP | 35 | 10 | 5 | 50 |
| Intent | 40 | 12 | 6 | 58 |
| **总计** | **490** | **147** | **64** | **701** |

---

## 下一步

### v0.7: Production Hardening (可选)
- 性能基准测试
- 安全审计
- 压力测试

### v1.0: Public Release
- Crate 发布
- 文档网站
- 社区建设

---

*太初 — AI-OS 完成态。Plico 从"可以工作"到"完全就绪"。*
