# Plico Benchmark Framework

标准化、工程化的 Plico AI-OS Kernel 评测体系。

## 快速开始

```bash
cd benchmarks
./scripts/setup.sh          # 安装依赖
uv run python -m plico_benchmarks list
uv run python -m plico_benchmarks run performance
```

## 架构

```
src/plico_benchmarks/
├── core/          # 基础设施（client, judge, metrics, embedding, llm, reporter）
├── datasets/      # 数据集加载器（自动回退到 legacy data）
└── suites/        # Benchmark 套件（conversational-qa, retrieval, kg-reasoning, performance）
```

## Suites

| Suite | 说明 | 状态 |
|-------|------|------|
| `conversational-qa` | LoCoMo + LongMemEval 对话记忆 QA | ✅ |
| `retrieval` | BEIR + MemoryAgentBench AR 检索精度 | ✅ |
| `kg-reasoning` | 知识图谱多跳推理 | ✅ |
| `performance` | CAS / Search / Memory / KG 微基准 | ✅ |
| `temporal-reasoning` | 时间推理专项 | 🚧 Skeleton |
| `memory-crud` | MemBench-style CRUD 正确性 | 🚧 Skeleton |

## 目录结构

| 路径 | 用途 |
|------|------|
| `benchmarks/` | 框架根目录，uv + pyproject.toml 管理 |
| `benchmarks/src/plico_benchmarks/` | Python 源码（core / datasets / suites） |
| `benchmarks/scripts/` | Shell 脚本（setup / run / model server launch） |
| `benchmarks/results/` | JSON 结果文件（Git 忽略具体内容） |
| `benchmarks/docs/` | 生成的 Markdown 报告 |
| `benchmarks/configs/` | YAML 配置文件 |
| `.runtime/bench_legacy/` | 旧 bench 目录完整备份（含数据） |

## 模型矩阵（llama.cpp 强制）

**禁止使用 Python sentence-transformers 做 embedding。** 所有推理必须通过 llama.cpp server 提供 OpenAI-compatible API。

| 模型 | 用途 | 端口 | GGUF |
|------|------|------|------|
| gemma-4-26B-A4B-it-Q4_K_M | LLM (judge + reader) | 18920 | 主模型 |
| Qwen3-Embedding-0.6B-Q8_0 | Embedding（默认） | 18921 | 1024 维，低资源 |
| v5-small-retrieval-Q4_K_M | Embedding（测试） | 18922 | Jina v5，检索专用 |
| bge-reranker-v2-m3-q4_k_m | Reranker | 18926 | 重排序 |

**教训**: BGE-m3 未获取到可用 GGUF（ModelScope 返回空文件/404），放弃。所有 embedding 模型统一通过 llama.cpp server 加载。

## 预处理阶段（AWB-like）

plicod 写入数据后**不会立即可搜索**。必须显式等待后台完成：
1. **Embedding 生成**（异步，由 embedding provider 处理）
2. **KG 提取**（`kg_builder` 后台线程，`triples=0 prefs=0` 日志标志完成）
3. **HNSW 索引刷新**

**正确流程**: `ingest all data → wait_for_indexing() → query`

**行业经验**（VDBBench、LanceDB Cloud Benchmark、llama-benchy）：
- 明确区分 `load/ingest` 和 `search` 阶段
- 报告 ingestion time + indexing completion time + query performance
- warmup 阶段默认启用；多次运行取 mean ± std
- 向量数据库 benchmark 必须在索引完全构建后才开始查询

**实现**: `PlicoClient.wait_for_indexing()` 使用 probe-based 轮询：写入一个 probe item，不断 search 直到能检索到它。这比固定 sleep 更可靠。

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
- plicod 每次 benchmark 前必须**全新启动**（`rm -rf ROOT`），消除状态污染。
- 脚本使用 `set -euo pipefail`，绝对路径解析（`SCRIPT_DIR` / `PROJECT_ROOT`）。
- Suite 失败时记录到 `FAILED_SUITES` 数组，不中断整体流程；运行后验证结果文件非空且含 `metrics`。
- `--dry-run` 预览配置；`--preprocess-timeout` 控制索引等待时间（默认 180s）。

## Embedding 模型配置

见 `configs/embedding_models.yaml`。5 个模型配置：bge-m3, bge-large-en-v1.5, jina-embeddings-v5, qwen3-embedding-0.6b, text-embedding-3-small。

### 已测试的 Embedding 模型

| 模型 | 端口 | 维度 | 量化 | 搜索 hit_rate | 搜索延迟 |
|------|------|------|------|--------------|---------|
| Qwen3-Embedding-0.6B | 18921 | 1024 | Q8_0 | **85-90%** | **14ms** |
| Jina v5-small-retrieval | 18922 | 1024 | Q4_K_M | **0%** | **140ms** |

**关键发现**：
- Qwen3-Embedding-0.6B 是当前最佳选择：搜索命中率 85-90%，延迟低（14ms），语义相似度区分度好
- Jina v5 GGUF 完全不可用：0% 命中率，10 倍延迟。原因可能是 GGUF 转换问题或 pooling 策略不匹配
- 切换模型只需改端口，plicod 重启即生效

## 多模型 Benchmark 运行

```bash
# 服务端口规划
# 18920: LLM (Gemma 4 26B)
# 18921: Embedding (Qwen3-0.6B)
# 18922: Embedding (Jina v5) — 目前不可用
# 18926: Reranker (bge-reranker-v2-m3)

# 启动所有服务
llama-server -m models/gemma-4-26B-A4B-it-Q4_K_M.gguf --port 18920 &
llama-server -m models/Qwen3-Embedding-0.6B-Q8_0.gguf --port 18921 --embedding --pooling mean &

# 运行 benchmark
cd benchmarks
PREPROCESS_TIMEOUT=600 ./scripts/run_full_benchmark.sh       # 全量
PREPROCESS_TIMEOUT=600 ./scripts/run_full_benchmark.sh --skip-jina-v5  # 仅 Qwen3

# 单 suite
./scripts/run_suite.sh performance
```

**超时注意**：conversational-qa suite 包含 LLM 推理（Gemma 4 26B），40 samples 可能需要 10+ 分钟。如果超时，设置 `PREPROCESS_TIMEOUT=600`。

## Benchmark Pipeline 经验

- **Search limit matters**: 5 → 15 snippets improved context hit rate significantly
- **Intent-specific prompts**: temporal/multi-hop questions need specialized prompts, not generic ones
- **F1 vs LLM Score**: F1 measures token overlap (low for paraphrased answers), LLM Score measures semantic correctness (better metric)
- **Context hit rate is the ceiling**: if search doesn't find the right content, no reader prompt can fix it

## 数据集

数据集自动从 `~/.cache/plico-benchmarks/` 加载。若本地有旧数据，会回退到 `../bench/data/` 或 `../.runtime/bench_legacy/data/`。

## 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `PLICO_HOST` | plicod 主机 | `127.0.0.1` |
| `PLICO_PORT` | plicod 端口 | `7878` |
| `EMBEDDING_API_BASE` | Embedding 端点 | `http://127.0.0.1:8080/v1` |
| `OPENAI_API_BASE` / `LLAMA_URL` | LLM 端点 | `http://127.0.0.1:8080/v1` |
| `PLICO_JUDGE_API_BASE` | Judge LLM 端点 | 同 LLM 端点 |
| `LLM_BACKEND` | LLM 后端 | `openai` |
| `PLICO_KG_AUTO_EXTRACT` | Benchmark 时关闭 KG 提取 | `false` |
| `PREPROCESS_TIMEOUT` | 索引等待秒数（脚本层） | `180` |

## Benchmark 数据

`benchmarks/results/` 下的 JSON 文件，版本号如 `v44`, `v45`, `v46`。
外部集成测试：`/home/leo/work/plico-agents` — CrewAI 多智能体系统通过 TCP 连接 plicod，测试 8 个子系统。
