# Benchmark 框架操作指南

Plico 的 benchmark 是独立 Python/uv 子项目。实际能力、环境变量和运行命令以
`benchmarks/README.md`、`benchmarks/configs/benchmark.yaml` 与 CLI `list` 的当前输出为准。

## 基础四套件

| Suite | 边界 |
|------|------|
| `performance` | public v2 对象、Working Memory、Projection、Session 与 Readiness 延迟 |
| `retrieval` | BEIR/MemoryAgentBench AR 上的 Plico object search 与 BM25 shadow 比较 |
| `memory-recall-lexical` | Working Memory lexical exact-contract recall，不得冒充语义/vector recall |
| `conversational-qa` | LoCoMo/LongMemEval research QA，DeepSeek reader/judge |

`v1b-release` 和 dogfood evidence 工具是发布正确性证据，不是质量或性能比较器。

## 运行边界

- 使用 `plico.personal.v2` exact-14 公共协议；不得恢复 v1 reader 或已删 operation。
- 每个 suite/run 必须使用独立 0700 fresh vault 和独立 plicod/UDS。
- `performance` 不接受统一 `--samples`；各 operation 的数量来自 YAML。
- 基础 smoke 使用 `--runs 1`；`--runs 5` 只产生 shadow 比较，不自动升级为 official gate。
- 单请求串行测得的 rate 只能报告为 `serial_service_rate`，不得写成并发 QPS。
- p99 样本不足门槛时只能是 exploratory diagnostic。

## 模型与付费请求

- Embedding 由 plicod 配置。只有每查询 `embedding_query=succeeded`、vector path
  `accepted > 0` 且零 degradation 时，才能声称 real-vector 测量。
- Reader/Judge 仅接受显式 `PLICO_READER_*` / `PLICO_JUDGE_*` DeepSeek role 配置；
  不读取旧 OpenAI/LLM 兼容变量，不回退到本地模型或其他 provider。
- API key 只注入 QA 子进程，不传给 plicod 或其它 suite，不得进入日志/artifact。
- 付费 attempt 必须在 I/O 前写 Prepared、I/O 后写 Finalized；费用与预算由
  owner-only durable journal 重放核销。transport 结果不确定时不自动重试。

## 证据与指标

- Memory 质量指标只能来自 `memory.recall`；object retrieval 不得命名为 Memory recall。
- Persisted ledger 要保留可重算的 opaque qrel/ranking/sample 证据，不得只保留自报聚合数。
- 降级、unsupported、stub 和 unavailable 必须单独计数，不得并入 measured success。
- 单次 run 不做比较推断。跨 run 结论需要相同 sample IDs、pipeline identity 与配对统计。
- `ragas_style_proxy` 是内部 proxy，不是官方 RAGAS；不得与外部 RAGAS 分数直接作差。

## 快速门禁

```bash
cd benchmarks
.venv/bin/ruff check .
.venv/bin/ruff format --check src tests
.venv/bin/python -m pytest -q -p no:cacheprovider
bash -n scripts/run_full_benchmark.sh
./scripts/run_full_benchmark.sh --dry-run --runs 1 --output-parent /tmp/plico-bench-plan
```

真实运行只在所需 dataset cache、DeepSeek role 配置和 embedding provider 均已 fail-closed
验证后启动。缺失任一前置时应停止，不使用 fallback 填补结果。
