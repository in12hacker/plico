# v53 R3 Freeze：WP3A Read Facade — Scope / Readonly Surface / Mutation Corpus

**日期**：2026-08-18
**冻结基线**：`cbef2f3`（R3 remediation，父 `0fd6061` = WP3A.2-B 交付）

## 1. 冻结提交链（WP3A exact scope）

| 提交 | 角色 | 内容 |
|---|---|---|
| `835c28e` | 架构 R2 acceptance | 基线 |
| `7957bba` | 开发 WP3A.1 | reader 五文件 + persisted-stamp 修复（P1-1） |
| `eefe7d8` | 架构 WP3A.2-A | existing-only readonly capability（CAS 侧冻结） |
| `0fd6061` | 开发 WP3A.2-B | reader 切 readonly closure、字符串分类删除、re-export、四项 reader 测试 |
| `cbef2f3` | 开发 R3 remediation | P2 收口：EOF diff-check 修复 + 偏差文档归档本目录 |

文件面（`git diff --stat 835c28e..cbef2f3`）：`execution_observation/mod.rs`（模块声明）、
`reader/{mod,replay,reducer,tests,readonly_tests}.rs`、`cas/execution_observation_store.rs`（+tests.rs，
架构冻结）、`cas/mod.rs`（单行 re-export）、`docs/milestones/v53-wp3a1-architecture-deviation.md`（归档）。

## 2. 冻结 readonly API surface（逐字）

```rust
pub(crate) struct ExistingExecutionObservationReadOnly<'a> { /* 私有字段，闭包有界 */ }

impl PersonalVaultStorage {
    pub(crate) fn with_existing_execution_observation_readonly<R>(
        &self,
        inspect: impl for<'a> FnOnce(Option<ExistingExecutionObservationReadOnly<'a>>) -> R,
    ) -> Result<R, LedgerStorageOpenError>;
}

impl ExistingExecutionObservationReadOnly<'_> {
    pub(crate) fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>>;
    pub(crate) fn get_immutable_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>>;
}
```

- 语义冻结：absent namespace → `inspect(None)`；present-but-damaged topology → typed fail-closed
  （`validate_existing_topology`）；永不 create/complete/chmod/claim；reader 与 writer 同 vault Arc 共存；
  只见完整 pre/post-exchange active pointer。
- `cas/mod.rs` 的 `pub(crate) use execution_observation_store::ExistingExecutionObservationReadOnly;`
  re-export 按交付态保留，处置权归架构组（R3 P2-2 后半项，未裁决）。

## 3. 冻结外部 mutation corpus（R3 十项，逐项证据）

全部在隔离 checkout（`git archive cbef2f3`、umask 077、外置 target、stub 后端）逐
`--exact --nocapture` 执行，均 rc=0 / `1 passed; 0 failed; 0 ignored`：

| # | 变异场景 | 载体（测试） | 类别断言 |
|---|---|---|---|
| 1 | persisted event generation 重哈希篡改 | `reader::tests::…tampered_stamp_generation_mismatch` | GenerationMismatch |
| 2 | alternate genesis（自洽重哈希） | `reader::tests::…rejects_alternate_genesis` | BrokenRootChain |
| 3a | duplicate Started | `reader::tests::reducer_rejects_duplicate_started` | DuplicateStarted |
| 3b | duplicate Terminal / rebind | `reader::tests::reducer_rejects_second_terminal_and_rebind` | DuplicateTerminal/InvalidTransition |
| 4 | Terminal without Started | `reader::tests::reducer_rejects_terminal_without_started` | InvalidTransition |
| 5 | current-view 自洽重哈希篡改 | `r3_view_probe`（外部探针，源码附录，不入树） | CurrentViewMismatch |
| 6 | fresh vault 零突变 | reader `…fresh_vault_reads_empty_and_mutates_nothing` + CAS `…readonly_fresh_vault_is_none_and_zero_mutation` | Ok/空链 + 指纹不变 |
| 7 | same-Arc reader/writer 共存 | reader `…never_burns_the_writer_claim` + CAS `…readonly_then_writer_same_arc` | writer 后开成功 / live claim 可读 |
| 8 | active exchange 完整快照 | reader `…snapshots_are_whole_during_publication` + CAS `…readonly_exchange_race_sees_whole_pointers` | 仅完整旧/新指针 |
| 9 | malformed/symlink/mode 零修复 | reader+CAS `damaged_topology…` + `…rejects_symlinked_slot…` + `…never_repairs_missing_or_special_slots` | typed fail-closed，模式不变 |
| 10 | 20,000/20,001 event 边界 | `reader::tests::…enforces_event_cap_and_generation_binding` | 恰 20,000 接受；20,001 → StoredResourceLimit |

注记：
- #10 边界由 reducer 钉死（`EVENTS_MAX=20_000` 唯一权威）；replay 步数界复用同一常量
  （`reader/replay.rs` parent 边计数、genesis 不计步）。
- #5 探针为本 R3 外部资产（等价于 verify_scope 外部语料的注入方式），源码附于本文件附录，
  不进入仓库树。
- 冻结语料（WP1 5 + WP2 17）在本 R3 树上复验 22/22；定向 execution_observation（含语料+探针）97/97。
- 观察申报：架构冻结语料文本自身含一处 `clone()`-on-Copy（`architecture_contract_tests.rs:29`），
  在 overlay 后的树上触发 clippy `-D warnings`；冻结树（无 overlay）clippy 零警告。属架构文本，
  开发组不修改，报请架构组知悉。

## 附录：R3 外部探针源码（current-view 自洽重哈希）

探针构造：exact genesis + 真实 Started event/segment，view 的 attempts[0].started_event_sha256
改为另一合法 hex64 谎言后重哈希 view→root→pointer 全链——所有哈希自洽，仅 reducer 重放比对可杀。
完整源码见 R3 验收工作区 `/tmp/plico-v53-r3-accept/src/memory/execution_observation/r3_view_probe.rs`
（SHA-256 以验收日志为准）；断言
`ObservationStoreError::corrupt(CorruptionCategory::CurrentViewMismatch)`。
