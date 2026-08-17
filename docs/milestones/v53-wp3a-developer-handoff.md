# v53 第三方开发交接：WP3A Deterministic Read Facade

**日期**：2026-08-18
**状态**：READY
**基线**：以本交接提交/tag 的 exact Git identity 为准

## 唯一任务

实现 ADR-0009 的 crate-private、只读 observation facade：从 ADR-0008 authoritative active chain 用唯一 pure reducer
重建 attempt state，并用重建结果验证 stored current view。不得实现 append、receipt、clock、repair 或生产接线。

## 允许范围

仅允许修改：

```text
src/memory/execution_observation/mod.rs
src/memory/execution_observation/reader/mod.rs
src/memory/execution_observation/reader/reducer.rs
src/memory/execution_observation/reader/replay.rs
src/memory/execution_observation/reader/tests.rs
```

需要修改 store/CAS/model/hash/validator、ADR/spec/verifier、Cargo、kernel/scheduler/API/bin 时立即停止并提交
Architecture Deviation。

## 必须证明

1. startup/read_attempt/current-view check 共用同一个 reducer；
2. 输入只能来自 validated authoritative chain，按严格连续 sequence；
3. Open/Terminal、同 execution 多 attempt、重启结果确定；
4. duplicate Started、Terminal without Started、cross-attempt rebind、第二个不同 Terminal 均 fail closed；
5. stored view tamper 即 `CorruptStore`，不信任或静默重建覆盖；
6. 20,000 event 上限；无 raw CAS/path/candidate/write/repair 能力；
7. default-off lifecycle 与 public operation catalog 字节级不变。

## 自证

先提交候选，再执行 repo 外 `CARGO_TARGET_DIR` 下的：

```bash
cargo fmt --all -- --check
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --offline --lib execution_observation_
cargo clippy --locked --offline --all-targets --all-features -- -D warnings
git diff --check <HANDOFF_BASE>..HEAD
```

交付 exact commit SHA、五文件 diff、原始测试摘要和已知限制。开发组不得宣称 R3 GO；架构组将补充独立 reducer
mutation corpus 后裁决。文件 `<300` 仍是目标而非 line-golf 指令；若单一内聚职责需要 301–500 行，提交理由，不扩大
visibility 或拆成无语义的 part 文件。
