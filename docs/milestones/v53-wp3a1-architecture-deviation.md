# Architecture Deviation：WP3A.1 读隔离前置缺失（已闭环）

- 日期：2026-08-18（闭环更新）
- 提交方：第三方开发组（P1-1 修复 candidate `7957bba5c10e03f4f0f43aeed1ff041de4165b5a`）
- 基线：R2 acceptance `835c28e335e5d04f241cd856cdaea9639a8befc0`
- 状态：**已闭环**。P1-1 由 `7957bba` 关闭；P1-2 前置由架构组 WP3A.2-A
  `eefe7d86f184a8504de2d803a5ed5110616c71b9`（existing-only readonly capability）交付，
  开发组极小适配（WP3A.2-B，本提交）按 §2 预案逐项落地，deviation 撤销。

## 0. 闭环记录（WP3A.2-B）

- `reader/mod.rs` `open_fixture` 切换至 `with_existing_execution_observation_readonly`
  闭包：absent namespace → 空链语义；present-but-damaged topology → typed
  fail-closed（reader 侧映射 `StorageUnavailable`，与 WP2 语料为 store open 阶段
  钉死的同一分类）；字符串 claim 分类删除（readonly 路径不再可能产生 claim 冲突）。
- `reader/replay.rs` 参数类型换为 `ExistingExecutionObservationReadOnly`，
  两读方法形状逐字相同，逻辑零改动；P1-1 stamp 修复原样保留（对抗测试 A 复验通过）。
- `src/cas/mod.rs` 补回 re-export（首个消费者落地）。
- reader 级测试四项：fresh 零突变（vault 指纹）、same-Arc writer 共存
  （reader 先开、writer 后开、live claim 下读回）、并发发布 whole-snapshot
  （barrier + 受控竞态循环）、damaged topology fail-closed 零修复。

## 1. 开发组对 WP3A.2 冻结设计的消费合同（从现有代码逐行提取，供架构组冻结参考）

reader 对 sealed CAS 的**全部**消费面（`reader/replay.rs`，已核实无其他调用）：

```rust
view.read_active_bounded(POINTER_MAX_BYTES as u64) -> std::io::Result<Option<Vec<u8>>>
view.get_immutable_bounded(hash: &str, max: u64)    -> std::io::Result<Vec<u8>>
```

恰好等于 WP3A.2-A 冻结 closure 暴露的两个方法——reader 无需 list、candidate、
put、publish、path 中的任何一项，适配前已是该形状的严格子集（冻结后逐字一致）。

## 2. 适配就绪性证明（已按此执行）

- `FixtureObservationReaderV1` 仅保留 `Vec<ReducibleAttemptV1>`，不持有
  storage/vault 句柄——与 closure-bounded 语义天然兼容：replay 的全部读取发生在
  `open_fixture` 内一次完成，closure 退出后 reader 继续服务不可变快照；
- 适配点唯一：`open_fixture` 的 opener 替换（已执行）；
- replay 签名只换参数类型；reducer/验证逻辑零改动（已执行，diff 18+/18- 纯替换）；
- P1-1 修复（persisted stamp 四重绑定 + 对抗测试 A）原样保留（已复验）；
- typed open error：字符串前缀分类已删除（已执行）。

## 3. 架构组硬约束清单（存档；reader 侧对应断言已全部落地）

不创建目录/slot；不 chmod/修复/发布；不登记或释放 writer claim；不通过
ImmutableLedgerStorage Drop 解锁；read handle 不逃出 closure；同 Arc 下 reader
前后 writer 可用；writer 已存在时 reader 只见完整旧链或新链；malformed/symlink/
缺 slot typed fail-closed 零修改；typed variant 匹配。六项最低反例由架构组 corpus
验证；reader 侧对应断言：fresh-vault 字节不变（指纹）、同 Arc writer 先后、
并发一致快照、damaged topology 零修复均已落地（见 §0）。

## 4. 执行顺序承诺（已履行）

定向 reader 测试 → clippy → 全库门禁；三路对抗审查（红队/测试审计/规范符合性，
发现项已收敛）；交付 exact commit + diff + 定向原始摘要；不宣称 R3 GO；WP3B 未触碰。
