# Plico 第三节点设计文档：多租户命名空间隔离

**版本**: v1.0
**日期**: 2026-04-19
**定位**: 第三节点核心功能架构设计

---

## 0. 背景与目标

### 0.1 第二节点成果

| 模块 | 版本 | 核心能力 |
|------|------|---------|
| F-2 主动上下文装配 | v9.0-M1 | DeclareIntent + 语义预装配 |
| F-3 AgentToken 认证 | v9.0-M1 | HMAC-SHA256 身份验证 |
| F-4 向量引擎升级 | v9.0-M1 | edgevec 二进制量化 |
| F-5 plico-sse 流式 | v9.0-M2/M4 | A2A 协议兼容 |

**灵魂对齐**: 94/100

### 0.2 第三节点目标

**多租户命名空间隔离**: 允许多个租户（团队/项目）在同一个 plico 实例中共享数据，但彼此隔离。

### 0.3 设计约束

1. **向后兼容**: 现有 `agent_id` 继续工作（无 tenant 字段的请求默认属于 "default" 租户）
2. **最小侵入**: 尽量复用现有机制，不重写核心模块
3. **安全性**: 租户隔离不可绕过，即使声称是 "kernel" agent

---

## 1. 现有架构分析

### 1.1 当前隔离模型

```
当前: agent_id 是顶级命名空间
─────────────────────────────
Agent A (team-alpha)  ─┐
Agent B (team-alpha)  ─┼─→ 共享数据 (via ReadAny/Shared Memory)
Agent C (team-beta)   ─┘

问题: 没有更高层级的隔离机制
```

### 1.2 各层隔离现状

| 层级 | 现有隔离机制 | 不足 |
|------|------------|------|
| CAS | SHA-256 CID 内容寻址 | 无租户隔离，内容全局去重 |
| Semantic FS | `created_by` ownership | 依赖 permission check，可被 ReadAny 绕过 |
| KG | `agent_id` ownership | 跨租户可见性无法控制 |
| Memory | `MemoryScope::Shared/Private` | 无租户边界 |
| Permission | `PermissionGuard` | trusted agent 可绕过所有检查 |

---

## 2. 多租户架构设计

### 2.1 核心概念

```
Tenant (租户)
  └─ Agent (智能体)
       └─ Intent (意图)
            └─ Object (CAS 对象 / KG 节点 / Memory Entry)

隔离策略:
  - 同一租户内的 Agent 可以共享数据（显式授权）
  - 不同租户之间完全隔离（即使声称 kernel 权限）
```

### 2.2 数据模型变更

#### 2.2.1 AIObjectMeta 新增 tenant_id

```rust
// src/cas/object.rs

pub struct AIObjectMeta {
    // ... existing fields ...
    pub tenant_id: String,    // 新增：租户 ID
}

impl AIObjectMeta {
    /// 默认租户（向后兼容）
    pub fn default_tenant() -> String {
        "default".to_string()
    }
}
```

#### 2.2.2 KGNode 新增 tenant_id

```rust
// src/fs/graph/types.rs

pub struct KGNode {
    // ... existing fields ...
    pub tenant_id: String,    // 新增：租户 ID
}

impl KGNode {
    pub fn new(label: String, node_type: KGNodeType, agent_id: String, tenant_id: String) -> Self {
        Self {
            // ... 
            tenant_id,
        }
    }
}
```

#### 2.2.3 PermissionContext 新增 tenant_id

```rust
// src/api/permission.rs

pub struct PermissionContext {
    pub agent_id: String,
    pub tenant_id: String,    // 新增：租户 ID
    pub embedded_grants: Vec<PermissionGrant>,
}

impl PermissionContext {
    pub fn new(agent_id: String, tenant_id: String) -> Self {
        Self {
            agent_id,
            tenant_id,
            embedded_grants: Vec::new(),
        }
    }

    /// 从 API token 推断租户（如果可用）
    pub fn with_inferred_tenant(agent_id: String, token_tenant: Option<String>) -> Self {
        Self {
            agent_id,
            tenant_id: token_tenant.unwrap_or_else(|| "default".to_string()),
            embedded_grants: Vec::new(),
        }
    }
}
```

#### 2.2.4 MemoryEntry 新增 tenant_id

```rust
// src/memory/layered.rs

pub struct MemoryEntry {
    // ... existing fields ...
    pub tenant_id: String,    // 新增：租户 ID
}
```

### 2.3 API 变更

#### 2.3.1 ApiRequest 新增 tenant_id 字段

所有需要身份验证的请求现在可以显式传递 `tenant_id`（可选，向后兼容）:

```rust
// src/api/semantic.rs

#[serde(rename = "create")]
Create {
    content: String,
    content_encoding: ContentEncoding,
    tags: Vec<String>,
    agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tenant_id: Option<String>,    // 新增
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_token: Option<String>,
    intent: Option<String>,
},
```

**向后兼容策略**:
- 如果 `tenant_id` 为 `None`，使用 token 中推断的租户或 "default"
- 如果 token 中也没有租户信息，使用 "default" 租户

#### 2.3.2 新增租户管理 API

```rust
// 创建租户
#[serde(rename = "create_tenant")]
CreateTenant {
    tenant_id: String,
    admin_agent_id: String,
},

// 列出可访问的租户
#[serde(rename = "list_tenants")]
ListTenants {
    agent_id: String,
},

// 租户间数据共享（需双方同意）
#[serde(rename = "tenant_share")]
TenantShare {
    from_tenant: String,
    to_tenant: String,
    resource_type: String,  // "kg" | "memory" | "cas"
    resource_pattern: String,  // tag pattern 或 "*"
},
```

### 2.4 租户隔离策略

#### 2.4.1 CAS 存储隔离

**设计决策**: CAS 内容（SHA-256 CID）本身不做租户隔离，因为:
1. 内容去重是核心特性，跨租户去重可以节省存储
2. 租户隔离通过 `AIObjectMeta.tenant_id` 实现

```
┌─────────────────────────────────────────────────────────┐
│  CAS Storage (全局共享内容，去重)                         │
│  ┌─────────────────────────────────────────────────────┐ │
│  │ CID: abc123... (SHA-256)                           │ │
│  │ Meta: { tenant_id: "team-alpha", created_by: "A" } │ │
│  └─────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
         ↑
         │ 隔离检查：读取时验证 tenant_id 匹配
```

**隔离规则**:
- `create`: 使用请求中的 `tenant_id`
- `read`: 只返回 `tenant_id` 匹配的对象（除非有跨租户权限）
- `search`: 结果按 `tenant_id` 过滤

#### 2.4.2 KG 隔离

```
Tenant-A KG Nodes ─┐
                  ├─→ 可通过跨租户边连接（需显式授权）
Tenant-B KG Nodes ─┘
```

**隔离规则**:
- `kg_add_node`: 自动使用请求者的 `tenant_id`
- `kg_list_nodes`: 只返回本租户的节点
- `kg_add_edge`: 跨租户边需要 `CrossTenantEdge` 权限

#### 2.4.3 Memory 隔离

```
Tenant-A Memory Entries ─→ Private/Shared/Group
Tenant-B Memory Entries ─→ 完全隔离
```

**隔离规则**:
- `MemoryScope::Shared` 只在同一租户内共享
- 跨租户共享需要 `TenantShare` API

#### 2.4.4 Permission 隔离

**关键变更**: `trusted_agents` 不再能绕过租户隔离

```rust
// src/api/permission.rs

impl PermissionGuard {
    /// 检查跨租户访问权限
    pub fn check_tenant_access(
        &self,
        ctx: &PermissionContext,
        target_tenant: &str,
        action: PermissionAction,
    ) -> std::io::Result<()> {
        // 1. 同租户访问：正常权限检查
        if ctx.tenant_id == target_tenant {
            return self.check(ctx, action);
        }

        // 2. 跨租户访问：需要显式 CrossTenant 权限
        //    即使是 kernel/system trusted agents 也必须显式授权
        if !ctx.embedded_grants.iter().any(|g| g.covers(PermissionAction::CrossTenant)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Agent '{}' in tenant '{}' cannot access tenant '{}'. Need CrossTenant permission.",
                    ctx.agent_id, ctx.tenant_id, target_tenant
                ),
            ));
        }
        Ok(())
    }
}
```

### 2.5 认证与租户推断

#### 2.5.1 Token 扩展

```rust
pub struct AgentToken {
    pub agent_id: String,
    pub tenant_id: String,         // 新增：token 隐含租户
    pub token: String,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
    pub capabilities: Vec<String>,
}
```

#### 2.5.2 租户推断流程

```
┌─────────────────────────────────────────────────────────────┐
│                    租户推断流程                               │
├─────────────────────────────────────────────────────────────┤
│ 1. API 请求携带 tenant_id?     → 使用请求中的值               │
│ 2. API 请求无 tenant_id?       → 从 token 推断                │
│ 3. token 也无 tenant_id?       → 使用 "default" 租户           │
│ 4. 但 "default" 租户需要验证   → AgentToken 必须有效           │
└─────────────────────────────────────────────────────────────┘
```

### 2.6 向后兼容策略

#### 2.6.1 迁移路径

| 阶段 | 行为 |
|------|------|
| 初始 | `tenant_id` 可选，所有请求默认为 "default" 租户 |
| 迁移 | 现有 agents 继续工作，生成新 token 时自动分配租户 |
| 完成 | 所有生产请求必须携带有效 `tenant_id` |

#### 2.6.2 兼容性模式

```rust
pub enum TenantMode {
    /// 可选模式（初始兼容）
    Optional,
    /// 严格模式（生产环境）
    Strict,
}

impl AIKernel {
    pub fn set_tenant_mode(&self, mode: TenantMode) {
        self.tenant_mode = mode;
    }
}
```

---

## 3. 实现计划

### 3.1 Phase A：基础设施（~1周）

**目标**: 添加 tenant_id 字段到核心数据结构

| 文件 | 变更 |
|------|------|
| `src/cas/object.rs` | `AIObjectMeta` 新增 `tenant_id` |
| `src/api/permission.rs` | `PermissionContext` 新增 `tenant_id`，`check_tenant_access` |
| `src/fs/graph/types.rs` | `KGNode` 新增 `tenant_id` |
| `src/memory/layered.rs` | `MemoryEntry` 新增 `tenant_id` |

**风险**: 低（additive 变更）

### 3.2 Phase B：API 路由（~1周）

**目标**: 实现租户推断和隔离检查

| 文件 | 变更 |
|------|------|
| `src/api/semantic.rs` | 所有请求支持 `tenant_id` 可选字段 |
| `src/kernel/mod.rs` | 租户推断逻辑 |
| `src/kernel/ops/fs.rs` | 隔离检查 |
| `src/kernel/ops/graph.rs` | KG 隔离检查 |
| `src/kernel/ops/memory.rs` | Memory 隔离检查 |

**风险**: 中（需要修改多处 ownership 检查逻辑）

### 3.3 Phase C：租户管理 API（~1周）

**目标**: 完整的租户管理功能

| 文件 | 变更 |
|------|------|
| `src/api/semantic.rs` | 新增租户管理 API |
| `src/kernel/mod.rs` | 租户管理 handler |
| `src/kernel/ops/tenant.rs` | 新增租户操作 |

**风险**: 低（新增 API，不影响现有功能）

### 3.4 Phase D：安全强化（~1周）

**目标**: 确保租户隔离不可绕过

| 文件 | 变更 |
|------|------|
| `src/api/permission.rs` | 移除 trusted agent 的租户绕过特权 |
| `src/kernel/mod.rs` | 所有 kernel 操作也需租户验证 |

**风险**: 高（可能影响现有 trusted agent 行为，需仔细测试）

---

## 4. 验证指标

| 指标 | 目标 | 测量方法 |
|------|------|----------|
| 租户隔离完整性 | 0 cross-tenant leaks | 自动化渗透测试 |
| 向后兼容性 | 100% 现有 API 继续工作 | 回归测试套件 |
| 性能影响 | <5% 延迟增加 | benchmark 对比 |

---

## 5. 与灵魂文档的对齐

| system.md 关键原则 | 第三节点对应实现 | 对齐度 |
|-------------------|----------------|--------|
| "智能体是第一公民" | Agent 属于 Tenant，Tenant 定义隔离边界 | ✅ |
| "Permission & Safety Guardrails" | 租户隔离作为权限系统扩展 | ✅ |
| "向后兼容" | 默认租户 + 可选 tenant_id | ✅ |

---

## 6. 技术选型备注

| 领域 | 选型 | 依据 |
|------|------|------|
| 租户标识 | 字符串 (UUID 格式) | 简单、可读、无状态 |
| 跨租户共享 | Tag pattern matching | 灵活、符合语义索引哲学 |
| Token 扩展 | 在现有 AgentToken 中添加 tenant_id | 最少侵入 |

---

*文档状态：指导性设计文档（Tier B）。实现细节以 Tier A（AGENTS.md + INDEX.md + 代码）为准。*
