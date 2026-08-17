# v53 第三方开发交接：WP2 Durable Store Substrate

本页是 `plico.milestone.v53.wp2/1` 的开发组唯一入口。它只授权
[ADR-0008](../adr/0008-execution-observation-store-substrate-v1.md) 与
[WP2 checkpoint](./v53-wp2-checkpoint.md) 的 durable substrate，不授权 WP3 facade/replay。

## 你会收到

- Git 仓库；
- 架构 approval commit A2 的 SHA；
- 派生 lightweight tag `v53-wp2-v1-<digest>`；
- 本页和 WP2 checkpoint。

你不会收到 architecture packet，也不运行正式 authorize/scope。你提交的测试只算 candidate self-evidence；
R2 由架构组在私有、离线 runner 独立验收。

## 开始方式

```bash
git switch -c v53-wp2-store <A2_SHA>
git merge-base --is-ancestor <A2_SHA> HEAD
```

只在 packet-frozen exact scope 内工作。不得修改合同、ADR、summary、spec、scope 工具、旧 WP1 类型/validator、
高扇入 `cas/ledger_store.rs`、`cas/mod.rs` 或 `cas/execution_observation_store.rs`。若 observation `mod.rs`
需要的变换与 checkpoint 不完全一致，立即停止，不要自行“等价实现”。

## 开发任务

1. 使用架构组冻结的 sealed CAS capability；
2. 新建 private `execution_observation::store` 五文件；
3. 实现 bounded loader、slot classifier、publisher 与 F06；
4. 所有 stored-read semantic/limit errors 在 loader boundary 映射 CorruptStore；
5. 不实现 receipt/current view/attempt facade，不接生产路径。

每个文件单概念；本 WP2 窄切片的交付上限为 `<300` 行，但禁止用 `part1/part2`、无语义的 helper、额外
`pub(super)`/`pub(crate)` 或跨文件往返来凑数。单一内聚职责确需超限时提交 Architecture Deviation，不要自行
拆散。禁止 `unsafe`、新依赖、raw path、环境变量、网络、background task、自由文本 error/tracing。测试可以
使用架构提供的 `#[cfg(test)]` fault seam，但不得把它暴露到 production constructor。
candidate tests 不得直接使用 `std::fs/os/path` 破坏物理状态；权限、symlink、raw corruption 等场景由架构组的
隔离 external corpus 注入。开发测试只能通过冻结 store/CAS seam 与 `tempfile` 创建的空 vault 观察行为。

## 本地 self-evidence

输出必须放仓库外；缺离线 cache 就停止并报告：

```bash
export CARGO_NET_OFFLINE=true EMBEDDING_BACKEND=stub LLM_BACKEND=stub
export PYTHONDONTWRITEBYTECODE=1
export CARGO_TARGET_DIR=<OUTSIDE_REPO>/cargo-target
export UV_CACHE_DIR=<OUTSIDE_REPO>/uv-cache
export UV_PROJECT_ENVIRONMENT=<OUTSIDE_REPO>/benchmark-venv

cargo fmt --all -- --check
cargo check --locked --offline --all-targets --all-features
cargo test --locked --offline --lib execution_observation -- --nocapture
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
git diff --check
git status --short
```

至少提供一个真实 `execution_observation_f06_*` test。空测试、ignored、仅打印“ok”、修改测试框架或开发者
自制 packet 不构成证据。

## 提交

提交一个 clean candidate commit C2，并给架构组：C2 SHA、A2 SHA、分支名、F06/targeted test计数和已知环境
限制。不要发送 absolute path、Home/用户名、凭据、业务正文、完整环境、packet 或运行时 secret。

## 立即停止条件

需要 current view/receipt/replay、修改 WP1 wire/hash、修改共享 CAS 实现、引入 public API、调用
`PersonalVaultStorage::open`、使用无界 CAS 方法、自动修复 topology、联网获取依赖或任何 scope 外文件时，立即
提交 Architecture Deviation，不要 workaround。
