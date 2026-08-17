# v53 第三方开发交接：WP2-R2 Durable Store Remediation

**日期**：2026-08-17
**状态**：等待 B3/A3 冻结；A3 发布前不得开工
**合同**：`plico.milestone.v53.wp2-r2/1`
**唯一任务**：修复 C2 `f60eec14da37b107a595f9f93e739a6c06bd6672` 在 R2 暴露的 structural-store 根因缺陷

本页是开发组的唯一入口。规范细节见 [WP2-R2 checkpoint](./v53-wp2-r2-checkpoint.md) 和
[ADR-0008](../adr/0008-execution-observation-store-substrate-v1.md)。[WP3 blueprint](./v53-wp3-blueprint.md) 仅是 Draft，
不在本次授权内。

## 1. 开工输入

架构组会单独交付：

- exact A3 commit SHA；
- lightweight tag `v53-wp2-r2-v1-<COMMITTED_SHA256>`；
- 本页与冻结的 `wp2_spec.json`。

开发组不接收 packet，不运行 `collect.py`/`authorize.py`/formal `verify_scope.py`，不得修改 tag 或
approval。从 exact A3 新建分支，把 C2 的有效实现思路重放为新 commit；不 merge/cherry-pick 整个 C2，
避免将已知越界字节一并带入。

## 2. 提交前第一门：shared self-preflight

在跑重型 Rust 测试前，先对已提交 Git bytes 执行：

```bash
python3 -B scripts/milestones/v53/developer_preflight.py \
  --repo . \
  --base <A3_COMMIT> \
  --candidate HEAD \
  --require-clean
```

返回 `status=PASS` 只表示候选通过与 formal gate 共用的静态规则；输出固定为
`self_evidence_only=true`、`authorization=unverified`、`gate_eligible=false`。禁止复制一份简化 grep 或修改
preflight 让自己通过。任何 issue 先消除根因，再运行后续门禁。

## 3. exact developer scope

只允许修改：

```text
src/memory/execution_observation/mod.rs
src/memory/execution_observation/store/mod.rs
src/memory/execution_observation/store/loader.rs
src/memory/execution_observation/store/publisher.rs
src/memory/execution_observation/store/slots.rs
src/memory/execution_observation/store/tests.rs
```

CAS、ADR、checkpoint、spec、verifier、preflight、summary、Cargo/lock 和 WP1 model/hash/validator 都是 architecture-owned。
需要改动其中任何文件时立即停止并提交 Architecture Deviation。

## 4. 必须一次性解决的根因

1. 一把 typed mutex 覆盖 poison check→head snapshot→validation→object writes→publish→state update 全事务；
2. persisted model 用 private `Parsed* → Validated*` typestate，只有 `Validated*` 能提供下一次 CAS lookup 的引用；
3. 所有 active chain 结束于重算的 exact G0 root SHA，拒绝 hash 自洽但非冻结 G0；
4. pointer 自身错误与 dual-slot 关系错误精确分类，stored corruption 不泄漏 caller error 或 I/O 错误；
5. 删除 publisher 在 `publish_active` 成功后的额外 `storage.flush()`；
6. 删除 production 自定义日志宏和 stdout/stderr/panic placeholder；本阶段不补 tracing；
7. 保持严格 sealed surface：`store/mod.rs` 只暴露 ADR 冻结 seam，child `pub(super)` 只做 store-private 协作。

CAS bounded atomic collision 修复已在 B3 中由架构组提供；开发组不要另做一层 pre-read 或 generic put。

## 5. 必须保留的语义

- active 始终 authoritative；candidate 不 promote；
- fresh `E/P(G0)` 只在 candidate bytes 与重算 exact genesis pointer 逐字节一致时重跑 publish；
- pre-exchange 失败为 `StorageUnavailable`，active 不变；post-exchange roots sync 不确定为
  `CommitIndeterminate` 并 poison 当前 handle；
- reopen 只全量验证 active，不继承内存 poison，不自动选 newest/orphan/candidate；
- WP2 不实现 attempt reducer、append/read/receipt、clock、live producer 或任何 public API。

## 6. 开发自证

preflight PASS 后，使用 repo 外的全新 `CARGO_TARGET_DIR`，至少执行：

```bash
cargo fmt --all -- --check
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --offline --lib --all-features
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
git diff --check <A3_COMMIT>..HEAD
```

候选提交后再跑一次 preflight，然后 push 开发分支/PR。交付：exact commit SHA、变更摘要、自证原始输出、
已知限制。开发组不宣称 R2 GO；架构组在私有 packet 和隔离 runner 中执行 formal scope 与 external corpus。

## 7. 立即停止条件

需要改 CAS/spec/verifier/ADR；需要增加依赖、path/namespace、crate-wide writer、第二 vault lock、recovery/repair；
需要接入 kernel/scheduler/API/bin；或 preflight 与本合同冲突。遇到任一项只上报 Architecture Deviation，
不自行 waiver。
