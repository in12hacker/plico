# v53 第三方开发交接：WP1

本页是 `plico.milestone.v53/2` 的开发组入口。它不扩大
[ADR-0007](../adr/0007-execution-observation-ledger-v1.md) 或
[v53 合同](./v53-execution-observation-spine.md)的范围。

## 双方边界

- 开发组只接收 Git 仓库、审批提交 A 的 SHA/tag，以及 WP1 允许文件清单；只提交一个以 A 为祖先的 Git candidate。
- R0 四文件 packet 不交给开发组。packet 由架构组保存在受控 runner，用于正式 `authorize`/`verify_scope`；开发组没有 packet 时无法自授权，这是预期行为，不妨碍本地编译、测试和提交候选。
- 开发组运行的结果都是 `candidate self-evidence`，不能自称 R1 GO、Architecture Accepted 或 production ready。
- 架构组在受控、离线 runner 上取得 candidate 后，独立执行 packet、Git approval、scope、外部 corpus 和 review。

## 开发起点与唯一范围

从架构组给出的 v2 审批提交 A 建分支。R0 只授权 WP1，允许改动：

```text
src/memory/mod.rs
src/memory/execution_observation/mod.rs
src/memory/execution_observation/ids.rs
src/memory/execution_observation/model.rs
src/memory/execution_observation/canonical.rs
src/memory/execution_observation/hash.rs
src/memory/execution_observation/validation.rs
src/memory/execution_observation/error.rs
src/memory/execution_observation/tests.rs
```

`src/memory/mod.rs` 只能新增一行 `pub(crate) mod execution_observation;`。不得修改合同、ADR、summary、
R0 工具、Cargo/CAS/store/current-view/fault/kernel/API/bin 或任何其他路径；不得新增 I/O、运行时接线或公共导出。

## 开发组本地自测顺序

先创建仓库外私有目录；示例中的路径由开发组自己选择，不得写入提交、日志包或 JSON：

```bash
export CARGO_NET_OFFLINE=true EMBEDDING_BACKEND=stub LLM_BACKEND=stub
export PYTHONDONTWRITEBYTECODE=1
export CARGO_TARGET_DIR=<OUTSIDE_REPO>/cargo-target
export UV_CACHE_DIR=<OUTSIDE_REPO>/uv-cache
export UV_PROJECT_ENVIRONMENT=<OUTSIDE_REPO>/benchmark-venv

cd benchmarks
uv sync --locked --offline --extra dev
uv run --offline --no-sync ruff check src tests
uv run --offline --no-sync ruff format --check src tests
uv run --offline --no-sync pytest -q
cd ..

cargo fmt --all -- --check
cargo check --locked --offline --all-targets --all-features
cargo test --locked --offline --lib --all-features
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
git diff --check
git status --short
```

先设置外置目录，再运行 uv/Cargo；不要在仓库内生成 `target/`、`.venv/`、cache 或 Python bytecode。
缺少离线依赖时停止并报告架构组，禁止联网补包或自行改锁文件。

## 提交给架构组

只需提供 candidate commit SHA、父链中审批 A 的 SHA、分支名和上述自测的简短计数。不要发送 R0 packet、
绝对路径、用户名/Home、凭据、真实个人数据、完整环境变量或原始业务正文。架构组会从 Git objects 重建候选，
所以未跟踪文件、工作树产物和开发者自制 packet 都不构成证据。

## 停止条件

遇到范围外文件、公共 API、CAS namespace/store、运行时 producer、真实身份/evidence verdict、联网依赖、
合同歧义或任一红门，立即停止并提交 Architecture Deviation；不得用空测试、waiver 或降级命令绕过。
