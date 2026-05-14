# 开发流程规范（详细说明）

本文件是 CLAUDE.md 中开发流程规范的展开版，提供更详细的说明和示例。

## 里程碑开发流程

### 阶段 1：里程碑规划

1. 从 `docs/milestones/TEMPLATE.md` 复制模板
2. 填写：目标、任务拆分、验收标准、风险评估
3. 每个任务必须有明确的"完成"定义（验证标准）
4. 提交给用户确认后才能进入开发

### 阶段 2：模块开发

- **按功能模块逐个开发**，不可一次性生成全部代码
- 每个模块必须覆盖测试用例
- 新增代码覆盖率 ≥ 90%（全局 `cargo llvm-cov --lib` ≥ 90%）
- 不可覆盖的模块用注释标注：
  ```rust
  // coverage:skip <reason>
  // 例如：// coverage:skip requires external LLM service
  ```
- 每个模块完成后立即运行质量门控

### 阶段 3：质量门控（每个模块完成后）

```bash
# 1. 全量 lib 测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 2. Clippy 无新增警告
cargo clippy -- -D warnings

# 3. 无 O(n²) 算法
# 关键路径需要性能测试验证
```

### 阶段 4：里程碑验收（所有模块完成后）

```bash
# 1. 回归测试：全量测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# 2. 覆盖率 ≥ 90%
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# 3. 性能回归测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression

# 4. Clippy 无新增警告
cargo clippy -- -D warnings
```

### 阶段 5：端到端测试

- 运行 benchmark suite 验证业务流程
- 生成 benchmark 报告到 `benchmarks/results/`
- 对比上一版本报告，确认无退化
- 推演下一阶段里程碑

## 退化判定规则

以下任一条件成立即判定为退化，里程碑开发失败，必须重新进入开发阶段修正：

| 条件 | 检测方法 | 基线 |
|------|---------|------|
| 新增测试失败 | `cargo test` | 里程碑前的测试结果 |
| 覆盖率下降 | `cargo llvm-cov --lib` | 里程碑前的覆盖率 |
| 性能回归 | `cargo test --test perf_regression` | `tests/perf_regression.rs` 中的阈值 |
| Clippy 新增警告 | `cargo clippy -- -D warnings` | 里程碑前的警告数 |
| Benchmark 指标下降 | 对比 `benchmarks/results/` | 上一版本的 benchmark 报告 |

## 版本快照存储规则

版本特定的数据（测试数量、覆盖率、QPS、benchmark 结果等）**禁止写入 CLAUDE.md**。

存储位置：`docs/milestones/vXX-summary.md`

格式模板见 `docs/milestones/TEMPLATE.md` 的第 7 节。

## 测试文件管理

测试文件**禁止随意生成**在项目根目录或不相关的目录中。

规则：
- **单元测试**：`#[cfg(test)] mod tests` 写在源文件内
- **集成测试**：写在 `tests/` 目录下，命名 `{module}_test.rs`
- **性能回归测试**：写在 `tests/perf_regression.rs`
- **Benchmark 脚本**：写在 `benchmarks/scripts/` 或 `benchmarks/src/` 下
- **临时测试文件**：用完立即删除，不提交到 Git

## 测试覆盖率门控

- **全局门控**：`cargo llvm-cov --lib` ≥ 90%
- **不可覆盖的模块**：在代码中用注释标注原因
  ```rust
  // coverage:skip requires external LLM service
  // coverage:skip requires running daemon
  // coverage:skip hardware-dependent (GPU detection)
  ```
- **Stub 后端限制**：stub embedding 返回空向量，语义搜索可能返回 0 结果。测试中不要断言搜索结果数量，用 `assert!(resp.ok)` 即可。

## 性能回归测试标准

**文件**: `tests/perf_regression.rs`

**设计原则**：
1. **确定性**：stub 后端，无外部依赖，CI 可运行
2. **快速**：< 2s 完成
3. **阈值驱动**：P50/P95 超标即失败
4. **捕获回归**：删除优化会导致测试失败

**阈值基线**：

| 操作 | 数据集 | P50 阈值 | P95 阈值 |
|------|--------|----------|----------|
| HNSW search | 100 vectors | < 1ms | < 5ms |
| HNSW search | 1000 vectors | < 5ms | < 15ms |
| HNSW search | 5000 vectors | < 10ms | < 30ms |
| HNSW upsert | single | < 1ms | < 5ms |
| HNSW delete | single | < 2ms | < 10ms |
| CAS write+read | single | < 20ms | < 50ms |
| Memory recall | 100 items | < 5ms | < 20ms |
| Search pipeline | 50 docs | < 20ms | < 100ms |
| Batch create | 50 items | < 80ms | < 200ms |
| KG find_paths | 20-node star | < 10ms | < 30ms |

## O(n²) 检测

对于大规模数据，以下模式是性能杀手：

- **`content.to_lowercase()` 在循环内** — 每次迭代分配新 String。使用零分配 `case_insensitive_contains()`
- **未排序的候选迭代** — 先按相关性/重要性排序，截断到 top-K 后再做昂贵操作
- **`Vec::push` 不预分配** — 已知大小时用 `Vec::with_capacity()`
