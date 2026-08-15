# Plico Benchmark Framework

标准化、工程化的 Plico AI-OS Kernel 评测体系。

## 快速开始

```bash
cd benchmarks
./scripts/setup.sh          # 安装依赖
uv run python -m plico_benchmarks list
uv run python -m plico_benchmarks run performance --seed 42
```

## 架构

```
src/plico_benchmarks/
├── core/          # 基础设施（client, judge, metrics, llm, reporter）
├── datasets/      # 只读显式填充并校验的本地数据缓存
└── suites/        # Benchmark 套件（质量、生命周期、隔离、性能）
```

## Suites

| Suite | 说明 | 状态 |
|-------|------|------|
| `conversational-qa` | LoCoMo + LongMemEval 对话记忆 QA | ✅ |
| `retrieval` | BEIR + MemoryAgentBench AR 检索精度 | ✅ |
| `performance` | 14 项公共协议中的对象、Working Memory、Projection、Session、Readiness E2E 性能 | ✅ |
| `memory-recall-lexical` | Working Memory lexical exact-contract recall | ✅ |

`v1b-release` 仍作为发布证据 suite 保留，但不属于基础四套件的质量/性能比较。

## 目录结构

| 路径 | 用途 |
|------|------|
| `benchmarks/` | 框架根目录，uv + pyproject.toml 管理 |
| `benchmarks/src/plico_benchmarks/` | Python 源码（core / datasets / suites） |
| `benchmarks/scripts/` | Shell 脚本（setup / run / model server launch） |
| `benchmarks/results/` | JSON 结果文件（Git 忽略具体内容） |
| `benchmarks/docs/` | 生成的 Markdown 报告 |
| `benchmarks/configs/` | YAML 运行配置与 judge prompt |

## 模型边界

Benchmark 不内置 Embedding provider，也不自动下载模型。Embedding 由本次运行的
`plicod` 配置并在 artifact 中按实际 execution path 报告；未验证或降级路径不得写成
real-vector 结论。Conversation QA 的 reader/judge 只接受显式、fail-closed 的 DeepSeek
role 配置，不回退到本地模型或其他 provider。

## 预处理阶段（AWB-like）

SemanticFS 的 `create → search` 评测必须分离摄取和查询阶段，等待其 Embedding、KG 与 HNSW 派生工作完成。Working Memory 是另一条数据域和索引管线，不能用 SemanticFS 的搜索结果判断其 projection 已 Ready。

**行业经验**（VDBBench、LanceDB Cloud Benchmark、llama-benchy）：
- 明确区分 `load/ingest` 和 `search` 阶段
- 报告 ingestion time + indexing completion time + query performance
- warmup 阶段默认启用；多次运行取 mean ± std
- 向量数据库 benchmark 必须在索引完全构建后才开始查询

`PlicoClient.wait_for_object_indexing()` 的 probe 只用于 public `object.put → object.search` 预处理。Working Memory 使用 `projection.status(kind=memory_embedding, revision_id=...)` 对每个 revision 做有界轮询，分别报告 `observed` 六态、`unreconciled`、`unavailable`、timeout、请求数和阶段耗时；两条派生管线不互相替代。

## V1-B 发布证据

`v1b-release` 是单次、破坏性的本地生命周期 run，不是模型或实现间比较器。它启动真实
`plicod`，通过 owner-only UDS 执行 `plico.personal.v2` exact-14 capability catalog 与 canonical
create/update/delete，验证 stale expected-head conflict、幂等 tombstone、重启 replay，以及 stub
provider 被 `projection.status` 诚实报告为 `unavailable(identity_unavailable)` 时 canonical read 仍成立；
owner rebuild 同样返回 typed dependency failure 且不改变 canonical。随后真实执行
`plico-memory-migrate inspect/dry-run/migrate`，并用 TCP
角色凭据和 UDS owner 验证 Private、Shared cutoff 与 Group mapping 的逐 stream recall 策略。
断连 fault ledger 覆盖 v2 的七类写操作：object put、memory create/update/delete、owner
projection rebuild、session start/end；每类只允许一个 request frame，响应丢失后不自动重放。

```bash
cargo build --features offline-migration --bin plicod --bin plico-memory-migrate
cd benchmarks
EMBEDDING_BACKEND=stub LLM_BACKEND=stub \
  PLICO_BENCH_TRACE_OUTPUT=/tmp/plico-v1b-release.daemon.log \
  uv run python -m plico_benchmarks run v1b-release \
  --output results/v1b-release-local
```

结果按 protocol/canonical/projection/restart/migration/policy 分段记录 latency、请求/响应 bytes、
ledger bytes、generation 与 watermarks。stub 仅用于证明 canonical acknowledgement 不依赖派生
projection；该 run 不声称 projection 或 thermal 已完成。run manifest 绑定协议/schema、Plico
binary、workload/config/input digest、git commit+dirty content digest、后端、非识别性硬件信息与
source watermark。保存 JSON 时，同 run_id 的 owner-only sidecar 再写入结果 artifact bytes+SHA-256；
这是为避免结果文件自哈希循环而采用的 detached binding。单次 run 固定
`comparative_inference=not_available_single_run`，不产生统计优越性结论。
如需把另一次真实 reader dogfood run 作为外部证据绑定，显式提供
`PLICO_BENCH_EXTERNAL_READER_TRACE`、`PLICO_BENCH_EXTERNAL_READER_RUN_ID`、
`PLICO_BENCH_EXTERNAL_READER_BACKEND` 和 `PLICO_BENCH_EXTERNAL_READER_MODEL`。suite 会验证
trace 的 0600 权限、run_id、reader/report 完成事件和公共协议成功事件；该证据只进入
`external_evidence`，不会并入本次 40 个 scored samples 或伪装成同一统计 run。

### P3-A dogfood evidence producer

`dogfood-evidence` 是纯离线封装器：不启动 daemon、不读取 `.env`/credential，也不把 capture
里的布尔值或 digest 当成事实。CLI 必须显式接收已执行的 0700 plicod 副本、Plico 与
0600 UDS socket、plico-agents 源码根、`uv.lock`、daemon/reader JSONL、Ollama probe、privacy canary、四份
canonical checkpoint、v1 reject 前后 zero-state checkpoint 以及最终 live vault。所有 private
输入必须 0600；live source 允许同 uid 的 0775/0664，但禁止 world-write，并在输出中标成
`live_same_euid_non_world_writable`，不伪称 sealed。

checkpoint 先由 `collect-canonical-checkpoint` / `collect-v1-zero-state` 以 NOFOLLOW、NOATIME、
bounded same-fd 读取生成。Ollama 证据由 `collect-ollama-probe` 实际调用 tags/version/embed，
输出只保留 exact configured tag、唯一匹配、前后 immutable digest/version、shape/norm 和 typed
contract，不保留 endpoint、其它 model、token 或 response body。显式 `:latest` 合法，但仍须
full-tag exact match；producer 按 Rust 相同的 JCS/domain 重算 provider compatibility 与完整
BuilderSpec hash。

```bash
cd benchmarks
uv run python -m plico_benchmarks dogfood-evidence \
  --capture /tmp/plico-p3a.capture.json \
  --plicod-binary /tmp/plico-run/plicod \
  --uds-socket /tmp/plico-run/plico.sock \
  --plico-root .. --plico-agents-root /path/to/plico-agents \
  --uv-lock /path/to/plico-agents/uv.lock \
  --daemon-trace /tmp/plico-p3a.daemon.jsonl \
  --reader-trace /tmp/plico-p3a.reader.jsonl \
  --ollama-probe /tmp/plico-p3a.ollama.json --canary /tmp/plico-p3a.canary.json \
  --canonical-before-rebuild /tmp/canonical-1.json \
  --canonical-after-rebuild /tmp/canonical-2.json \
  --canonical-before-restart /tmp/canonical-3.json \
  --canonical-after-restart /tmp/canonical-4.json \
  --v1-zero-before /tmp/v1-zero-1.json --v1-zero-after /tmp/v1-zero-2.json \
  --canonical-vault /tmp/plico-vault --output-dir /tmp/plico-p3a.evidence
uv run python -m plico_benchmarks verify-dogfood-evidence \
  --artifact-dir /tmp/plico-p3a.evidence
```

输出是 O_EXCL 创建的 0700 directory；`evidence.json`、detached sidecar、`LOCK` 与最后提交的
`COMMITTED` 均为 0600，并逐文件及 parent fsync。验证器只接受 exact 四文件 complete pair，
重算 binary/source/trace/inventory/probe、exact-14、七断连、reader 四类真实检索、restart 后
get/recall/status 与 v1 pre-dispatch reject 的 zero-state。它拒绝 duplicate key、symlink/FIFO/special、
secret/path/full raw hash、并发混合和不完整 crash 目录。该证据提供本机 artifact 完整性与漂移检测，
不是外部密码学 attestation。

## 数据格式陷阱

| 数据集 | 陷阱 |
|--------|------|
| LoCoMo | `qa` 是 list，item 的 `answer` 可能是 `int`/`None`，必须用 `str()` 包裹后再 `.lower()` |
| LongMemEval | `haystack_sessions` 是 list of lists；`answer` 同理需 `str()` |
| BEIR | `corpus` 是 dict(id → doc)，`queries` 是 list of dicts，`qrels` 是 dict(qid → list of doc_ids)。只摄取前 500 个可能导致 qrels 映射失败 |
| MemoryAgentBench | `answers` 可能是 list 或 str，不能假设 `.lower()` 可用 |

## 进程与脚本规范

- **禁止多次 `nohup` 无序启动**。使用 PID 变量 + `trap cleanup EXIT`。
- 启动前验证 model server 健康（`curl /models`），避免 plicod 启动后因 embedding 不可用而崩溃。
- plicod 每个 suite/run 都使用新建且启动前为空的 owner-only 临时 vault，消除状态污染。
- 脚本使用 `set -euo pipefail`，绝对路径解析（`SCRIPT_DIR` / `PROJECT_ROOT`）。
- `run_full_benchmark.sh` 采用 fail-closed：任一 suite 失败即非零退出，不生成不完整 combined report；每个成功结果仍必须非空且含 `metrics`。
- `--dry-run` 预览配置；`--runs 1|5` 选择一次 smoke 或五次 shadow 重复；`--preprocess-timeout` 控制索引等待时间。

Benchmark 不保持独立 Python Embedding provider 或模型配置双轨；切换模型通过 plicod 的 `EMBEDDING_API_BASE`/`EMBEDDING_MODEL` 配置并重启生效。模型优劣只从带 run manifest 的同协议实测 artifact 得出，不在文档中固化无来源历史分数。

## 基础 Benchmark 运行

```bash
cd benchmarks

# 不启动服务、不付费，只检查四类基础 suite 的计划
./scripts/run_full_benchmark.sh --dry-run --runs 1 --output-parent /tmp/plico-bench-plan

# 默认一次 fresh-vault smoke；需要已冻结的 READER/JUDGE DeepSeek role 配置
PREPROCESS_TIMEOUT=600 ./scripts/run_full_benchmark.sh --runs 1 --output-parent /tmp/plico-bench

# 五次独立重复只生成 shadow 比较证据，不升级为 official
PREPROCESS_TIMEOUT=600 ./scripts/run_full_benchmark.sh --runs 5 --output-parent /tmp/plico-bench
```

四类基础 suite 是 `performance`、`retrieval`、`memory-recall-lexical`、`conversational-qa`。最后一项使用严格 DeepSeek reader/judge 角色配置；API key 只注入 QA 子进程，不传给 plicod 或前三类 suite。Memory vector/hybrid 仍为 typed unsupported，当前结果最多是 research/shadow evidence。

## Benchmark Pipeline 经验

- **Search latency has two workloads**: `search_warm_repeated` measures primed repeated queries; `query_cold_unique` measures unique query texts after the index is warm and includes query embedding. It is not a cold-start or cache-cold measurement. 两者独立报告，不生成会掩盖差异的混合聚合值。
- **Remote E2E threshold**: do not apply a single `p50 < 5ms` requirement when remote query embedding is enabled; set separate, deployment-specific SLOs for warm and cold workloads.
- **Serial rate, not QPS**: performance operations currently issue one request at a time, so throughput is reported as `serial_service_rate` with `rate_unit=requests/s`, not concurrent QPS.
- **Working Memory boundary**: `memory.create_ack` measures canonical persisted Working Memory acknowledgement. `projection.memory_embedding_catch_up` separately polls the typed per-revision manifest observation.
- **Public capability boundary**: benchmark 只调用 `plico.personal.v2` 的 14 项能力。`memory.index_status` 和 v1 reader 已删除；Memory vector/hybrid/BM25 recall 仍 unsupported。KG mutation/path、跨角色 shared scope、伪 L0/L1/L2 context、object update/delete/batch 均无公共契约，因此其旧 suite 已物理删除。
- **Neutral QA protocol**: 当前 reader 使用单一 neutral prompt，不按 gold category 路由或改写答案；协议尚未预冻结，因此最多是 research/shadow。
- **F1 vs LLM Score**: F1 measures token overlap (low for paraphrased answers), LLM Score measures semantic correctness (better metric)
- **Evidence recall is the ceiling**: 未召回标注证据时，reader prompt 无法补救；不能用“返回任意 context”替代证据命中。
- **Performance config source**: operation counts, seed counts, batch size and warm queries come from `configs/benchmark.yaml`. Performance 不接受统一 `--samples` 覆盖，避免不同 operation 被静默改成同一采样量。

## 可复现性与量纲

- 每个 JSON 结果保留请求/实际样本量、数据集或 operation 分布、随机种子、预处理超时、服务端地址及非敏感模型环境变量。performance 的 `samples_evaluated` 按非聚合、已实测 operation 的 `count` 求和。
- schema v4 在结果内嵌机器可校验的 `run_manifest`，并将 `result.json`、`run_manifest.json`、`LOCK` 与 `COMMITTED` 作为 owner-only committed directory 原子提交。`actual = scored + failed + excluded` 必须守恒，输入数据集与结果 artifact 均记录字节数和 SHA-256；协议、模型/后端与当前可用的 source watermark 状态也进入 manifest。
- 单次 suite 执行固定记录 `independent_runs_observed=1` 与 `comparative_inference=not_available_single_run`，不能把一次运行包装成跨运行显著性结论。比较结论必须来自后续独立重复 run 的配对统计。
- 已删除按文件名选择两个单次结果并直接相减的 `compare`/`--compare` 路径；在重复运行分析器实现 manifest 同条件校验、配对 bootstrap 与置信区间前，不生成跨 run delta。
- Artifact 中的模型服务 URL 仅保留 `scheme://host[:port]`；userinfo、path、query 和 token 不会写入结果。
- 默认随机种子为 `42`；通过 `--seed` 或 `PLICO_SEED` 覆盖。
- Plico 的 `ragas_style_proxy` 由自定义单 LLM judge prompt 生成，**不是**官方 RAGAS 实现，不能与已发布 RAGAS 分数直接作差。
- BEIR SciFact `recall@k` 与 MTEB 多任务平均分属于不同数据集、指标和尺度，不能直接比较或计算 gap。

## 数据集

数据集只从已显式填充并校验的 `~/.cache/plico-benchmarks/` 加载。加载器不执行网络下载，也不回退到旧目录；缺少或损坏数据时 fail closed。

## 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `PLICO_HOST` | plicod 主机 | `127.0.0.1` |
| `PLICO_PORT` | plicod 端口 | `7878` |
| `PLICO_BEARER_TOKEN` | plicod 的 personal-owner bearer；TCP benchmark 必填 | 未设置（fail closed） |
| `PLICO_KG_AUTO_EXTRACT` | Benchmark 时关闭 KG 提取 | `false` |
| `PREPROCESS_TIMEOUT` | 索引等待秒数（脚本层） | `300` |
| `PLICO_SEED` | 抽样与 proxy 评估的随机种子 | `42` |
| `PLICO_BENCH_RUN_CLASS` | manifest 的运行类别，仅 regression/research | `research` |
| `PLICO_BENCH_REQUIRE_REAL_EMBEDDING` | 为 1 时，stub、query degradation 或 projection 未全部 Ready 会使 run 失败 | 未设置 |
| `PLICO_BENCH_TRACE_OUTPUT` | V1-B 脱敏 daemon trace 的 owner-only 输出路径 | suite 临时目录 |
| `PLICO_BENCH_EXTERNAL_READER_TRACE` | 另一次真实 reader UDS trace；仅作为 linked evidence | 未设置 |

## Benchmark 数据

结果以独占 run directory 提交，并由 detached manifest 绑定；不再依赖可覆盖的
`<suite>_<version>.json` 文件名作为信任边界。本地 `benchmarks/results/` 仅用于开发产物，默认不入库。
