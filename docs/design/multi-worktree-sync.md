# Multi-Worktree Architecture & Data Synchronization

**Date**: 2026-04-16
**Status**: Design
**Owner**: Plico Self-Management

---

## 1. 问题背景

当前 Plico 开发使用多 worktree 并行：
- `/home/leo/work/Plico` — 主仓库（main 分支）
- `/home/leo/work/plico-self-management` — worktree（`feat/plico-self-management` 分支）

**核心问题**：多个 worktree 之间如何同步？各自的数据（KG/CAS/Memory）如何管理？

---

## 2. Git Worktree 核心原理

### 2.1 Worktree 共享模型（来源：Andrew Roat, 2022）

```
┌─────────────────────────────────────────────────────────────┐
│                    Git Repository (.git)                     │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  objects/  refs/  worktrees/  config  hooks/        │    │
│  │  ← 共享：所有 worktree 的 commit 对象都在这里 →    │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
         ↑                           ↑                        ↑
    Main worktree               Worktree A                Worktree B
  (Plico main dir)       (plico-self-management)       (future worktree)
  [git tracked files]    [git tracked files]           [git tracked files]
  [plico --root]        [plico --root]                [plico --root]
```

**关键点**（来源：[git-scm.com/docs/git-worktree](https://git-scm.com/docs/git-worktree)）：
- 所有 worktree **共享同一个** `.git` 目录（仓库）
- 任何 worktree 的 commit 都写入**同一个** objects 数据库
- `git worktree list` 显示所有 worktree 的路径、HEAD、分支
- Worktree 可以被 `lock`、`move`、`remove`、`prune`

**vs 多个 git clone 的对比**（来源：[Andrew Roat](https://andrewlock.net/working-on-two-git-branches-at-once-with-git-worktree/)）：

| 特性 | 多个 Clone | Git Worktree |
|------|-----------|--------------|
| Git 对象（.git） | ❌ 重复，每个 clone 有自己的 objects | ✅ 共享，同一个 objects 数据库 |
| fetch 同步 | ❌ 必须在每个 clone 重复 fetch | ✅ 一次 fetch，所有 worktree 立即可见 |
| 分支同步 | ❌ 必须 push/pull 才能共享 | ✅ 天然共享，commit 立即在所有 worktree 可见 |
| 工作目录 | ❌ 完全隔离 | ❌ 完全隔离 |

**结论**：Git worktree 完美解决了"多个 clone 的同步问题"——代码天然同步。

### 2.2 哪些被共享，哪些不共享

| 资源 | 是否共享 | 说明 |
|------|---------|------|
| Git commits（代码） | ✅ 共享 | 所有 worktree 共享同一个 objects 数据库 |
| Git branches | ✅ 共享 | HEAD 指针在所有 worktree 实时同步 |
| 工作目录文件（代码） | ❌ 独立 | 每个 worktree 有独立的文件系统视图 |
| `--root` 目录（KG/CAS/Memory） | ❌ 独立 | 由 `--root` 参数指定，不在 git 管理下 |
| Git config（per-worktree） | ⚠️ 可选 | `extensions.worktreeConfig true` 开启后可用 `--worktree` 配置 |

### 2.3 Bare Repo 模式（所有 worktree 完全平等）

来源：[dev.to](https://dev.to/recca0120/git-worktree-multiple-working-directories-per-repo-and-the-key-to-parallel-ai-agents-40)

```bash
mkdir myproj && cd myproj
git clone --bare git@github.com:org/repo.git .bare
echo "gitdir: ./.bare" > .git
git worktree add main
git worktree add feat-x
```

```
├── .bare/      # 实际的 Git 对象存储
├── .git        # 文件，指向 .bare
├── main/       # main 分支 checkout
└── feat-x/    # feat-x 分支 checkout
```

- 没有"主工作副本"——每个分支都是 worktree
- `cd` 就是分支切换
- 与 tmux + AI agents 完美配合

### 2.4 Worktree 检测（用于共享 --root）

来源：[GitHub Issue #34437](https://github.com/anthropics/claude-code/issues/34437)

当多个 worktree 需要共享同一个 `--root` 时，可用 git 命令检测主 worktree：

```bash
# 列出所有 worktree，识别主 worktree
git worktree list --porcelain

# 解析到共享的 .git 目录
git rev-parse --git-common-dir
```

这正是 Claude Code 解决 worktree 记忆碎片化问题的方法——检测 worktree 后解析到主仓库路径。

### 2.5 Worktree 锁机制

```bash
# 锁定 worktree（防止被 prune 清理）
git worktree lock --reason "active development" /path/to/worktree

# 解锁
git worktree unlock /path/to/worktree

# 清理失效的 worktree 引用
git worktree prune
```

**注意**：`lock` 只防止被 `prune` 清理，**不提供写锁**。单用户约定：同一时刻只有一个 plicod 写入 `--root`。

来源：[git-scm.com/docs/git-worktree](https://git-scm.com/docs/git-worktree)

---

## 3. Plico 数据隔离架构

### 3.1 当前设计：每个 Plico 实例独立

```
用户 A 的 Plico              用户 B 的 Plico           工作 tree C
┌────────────────┐         ┌────────────────┐      ┌────────────────┐
│ KG nodes       │         │ KG nodes       │      │ 代码（git）    │
│ CAS objects    │         │ CAS objects    │      └────────────────┘
│ Layered Memory │         │ Layered Memory │
└────────────────┘         └────────────────┘
      ↓                        ↓
  --root ~/.plico-a        --root ~/.plico-b      --root 由 plicod 指定
```

来源：[zep.ai](https://www.getzep.com/) 架构原则：每个用户/线程隔离的 memory graph。

### 3.2 当前开发模式

```
# 主仓库（Plico 主目录）— 用于日常开发
cd /home/leo/work/Plico
git checkout main  # 或任何开发分支

# Worktree（独立目录）— 用于实验性功能
git worktree add /home/leo/work/plico-self-management feat/plico-self-management

# 每个 worktree 运行独立的 plicod（使用不同 --root）
cargo run --bin plicod -- --root /tmp/plico-feature-x --port 787x
```

### 3.3 Worktree 间的数据同步

当前阶段（单用户开发）：
- **代码同步**：`git push` / `git pull` / `git fetch`（由 git 仓库自动同步）
- **Plico 数据同步**：不使用 git worktree 机制，由各 `--root` 独立管理

---

## 4. 未来：PR-Based 记忆同步

当多用户协作时，用户之间的 Plico 记忆（KG nodes）通过 git PR 同步：

```
用户 A 的记忆导出 PR：
┌──────────────────────────────────────────┐
│  git commit: KG node "plan-X" JSON      │
│  git commit: KG node "iter13" JSON      │
│                                          │
│  diff: 仅包含 KG/CAS/Memory 相关文件     │
│  不包含：业务代码（那是另一个 PR）       │
└──────────────────────────────────────────┘
        ↓ PR review
  通过后 merge to main
        ↓
  用户 B 的 plicod 启动时 import
```

### 4.1 导出：KG Nodes → Git Files

```rust
// 每个 KG node 序列化为一个 JSON 文件
// kg/nodes/iter12.json
// kg/nodes/plan-plico-self-mgmt.json
// kg/edges/iter12→plan-plico-self-mgmt.json

pub fn export_kg_to_git(&self, output_dir: PathBuf) {
    let nodes = self.kg.all_node_ids();
    for node_id in nodes {
        if let Some(node) = self.kg.get_node(&node_id).ok().flatten() {
            let path = output_dir.join("kg/nodes").join(format!("{node_id}.json"));
            // serde_json::to_writer_pretty(path, &node)
        }
    }
}
```

### 4.2 导入：Git Files → KG Nodes

```rust
pub fn import_kg_from_git(&self, input_dir: PathBuf) {
    // 读取 kg/nodes/*.json
    // 调用 kg.add_node() 重建 KG
}
```

---

## 5. Zep/Graphiti 参考架构

来源：[Zep: A Temporal Knowledge Graph Architecture for Agent Memory](https://arxiv.org/abs/2501.13956), [getzep.com](https://www.getzep.com/)

关键设计原则：

1. **Temporal Facts**：每个 fact 有有效时间范围（"2024-11-14 - present"），fact 变更时旧 fact 保留历史而非删除
2. **Thread Isolation**：每个用户/会话的 memory graph 隔离（对应 Plico 的 per-user --root）
3. **Automatic Entity Extraction**：实体和关系自动从数据中提取（Plico 的 semantic create/update）
4. **Fact Invalidation**：当数据变化时，旧 fact 被失效而非覆盖（Plico 的 soft delete + restore）

这与 Plico 的设计高度一致：
- Plico KG 已有 `created_at` 时间戳
- Plico 的 `list_deleted` / `restore` 提供了事实失效机制
- 未来需要为每个 KG edge 添加 `valid_from` / `valid_to` 时间戳

---

## 6. 立即行动计划

### 6.1 HTTP API — ✅ 已实现

plicod 现在在 `--http-port`（默认 7879）提供 HTTP API：

```bash
# 启动
cargo run --bin plicod -- --root /tmp/plico --port 7878 --http-port 7879

# 测试
curl http://localhost:7879/api/project/status
curl http://localhost:7879/health
```

### 6.2 Dashboard HTML — 待完成

Dashboard 在 `/home/leo/work/Plico/docs/dashboard/index.html`，需要：
1. 将 API URL 从 TCP socket 改为 HTTP `http://127.0.0.1:7879/api/project/status`
2. 将静态 JSON 结构改为适配 `project_status()` 返回的动态数据

### 6.3 测试验证

```bash
# 启动 plicod
cargo run --bin plicod -- --root /tmp/plico-test --http-port 7879

# 测试 HTTP API
curl http://localhost:7879/api/project/status | python3 -m json.tool

# 在 dashboard 打开 http://localhost:7879/dashboard/
```

---

## 7. 参考资料

- [git-scm.com/docs/git-worktree](https://git-scm.com/docs/git-worktree) — 官方文档
- [Andrew Roat: Working on two git branches at once with git worktree](https://andrewlock.net/working-on-two-git-branches-at-once-with-git-worktree/) — 多 clone vs worktree 对比
- [GitHub Issue #34437: Worktrees should share the same project directory](https://github.com/anthropics/claude-code/issues/34437) — Claude Code 记忆碎片化问题与解决方案
- [dev.to: git worktree + parallel AI agents](https://dev.to/recca0120/git-worktree-multiple-working-directories-per-repo-and-the-key-to-parallel-ai-agents-40) — bare repo 模式 + AI agents
- [Medium: Mastering Git Worktrees with Claude Code](https://medium.com/@dtunai/mastering-git-worktrees-with-claude-code-for-parallel-development-workflow-41dc91e645fe) — Claude Code + worktree 最佳实践
- [Zep: A Temporal Knowledge Graph Architecture for Agent Memory](https://arxiv.org/abs/2501.13956) — arXiv 2025
- [getzep.com](https://www.getzep.com/) — Zep 官方架构
- [CSDN: Git Worktree 最佳实践](https://blog.csdn.net/clwahaha/article/details/125865474)
- [Git Worktree 屠龙技](https://zhuanlan.zhihu.com/p/92906230)
- [DataCamp: Git Worktree Tutorial](https://www.datacamp.com/tutorial/git-worktree-tutorial)
