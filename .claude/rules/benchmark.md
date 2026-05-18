# Benchmark 框架操作指南

Plico 使用自研 benchmark 框架（`benchmarks/`）进行端到端性能与质量评估。
完整文档见 `benchmarks/README.md`。

## 目录结构

| 路径 | 用途 |
|------|------|
| `benchmarks/` | 框架根目录，uv + pyproject.toml 管理 |
| `benchmarks/src/plico_benchmarks/` | Python 源码（core / datasets / suites） |
| `benchmarks/scripts/` | Shell 脚本（setup / run / model server launch） |
| `benchmarks/results/` | JSON 结果文件（Git 忽略具体内容） |
| `benchmarks/docs/` | 生成的 Markdown 报告 |
| `benchmarks/configs/` | YAML 配置（benchmark.yaml, embedding_models.yaml, judge_prompts.yaml） |

## 模型矩阵（llama.cpp 强制）

**禁止使用 Python sentence-transformers 做 embedding。** 所有推理必须通过 llama.cpp server 提供 OpenAI-compatible API。

| 模型 | 用途 | 端口 | GGUF |
|------|------|------|------|
| gemma-4-26B-A4B-it-Q4_K_M | LLM (judge + reader) | 18920 | 主模型 |
| Qwen3-Embedding-0.6B-Q8_0 | Embedding（默认） | 18921 | 1024 维，低资源 |
| v5-small-retrieval-Q4_K_M | Embedding（测试） | 18922 | Jina v5，检索专用 |
| bge-reranker-v2-m3-q4_k_m | Reranker | 18926 | 重排序 |

## 预处理阶段（AWB-like）

plicod 写入数据后**不会立即可搜索**。必须显式等待后台完成：
1. **Embedding 生成**（异步，由 embedding provider 处理）
2. **KG 提取**（`kg_builder` 后台线程，`triples=0 prefs=0` 日志标志完成）
3. **HNSW 索引刷新**

**正确流程**: `ingest all data → wait_for_indexing() → query`

**实现**: `PlicoClient.wait_for_indexing()` 使用 probe-based 轮询：写入一个 probe item，不断 search 直到能检索到它。这比固定 sleep 更可靠。

## 数据格式陷阱

| 数据集 | 陷阱 |
|--------|------|
| LoCoMo | `qa` 是 list，item 的 `answer` 可能是 `int`/`None`，必须用 `str()` 包裹后再 `.lower()` |
| LongMemEval | `haystack_sessions` 是 list of lists；`answer` 同理需 `str()` |
| BEIR | `corpus` 是 dict(id → doc)，`queries` 是 list of dicts，`qrels` 是 dict(qid → list of doc_ids) |
| MemoryAgentBench | `answers` 可能是 list 或 str，不能假设 `.lower()` 可用 |

## 共享工具（core/）

| 模块 | 函数/类 | 用途 | 使用者 |
|------|---------|------|--------|
| `core/metrics.py` | `accuracy_pct(scores, threshold=4)` | LLM score ≥ 4 的百分比 | conversational-qa, memory-lifecycle, intent-routing, causal-reasoning |
| `core/metrics.py` | `token_level_f1()`, `bleu1()`, `exact_match()` | Token 级别文本相似度 | conversational-qa |
| `core/metrics.py` | `recall_at_k()`, `ndcg_at_k()`, `mrr()` | 检索质量指标 | retrieval |
| `core/metrics.py` | `compute_statistics()`, `latency_percentiles()` | 统计汇总（mean/std/CI/p50/p95/p99） | 全部 suite |
| `core/metrics.py` | `estimate_tokens()` | Token 数估算（英文 ~4 chars/token，CJK ~1 token/char） | token-efficiency, proactive-optimization |
| `core/metrics.py` | `aggregate_category()` | 按 category 聚合结果（含 accuracy_pct） | 需要分组统计的 suite |
| `core/judge.py` | `Judge.evaluate()` | 二元判断（correct/incorrect） | 通用 |
| `core/judge.py` | `Judge.evaluate_scored()` | 1-5 分制评分 | conversational-qa, memory-lifecycle, intent-routing, causal-reasoning |
| `core/judge.py` | `Judge.evaluate_ragas()` | RAGAS 4 指标（faithfulness/relevancy/precision/recall），0-10→0.0-1.0 | conversational-qa（20 项样本） |
| `core/judge.py` | `Judge.evaluate_batch()` | ThreadPoolExecutor 并发评估 | 高吞吐场景 |
| `core/competitors.py` | `get_memory_competitors()` | LongMemEval/LoCoMo/PersonaMem 基线 | conversational-qa, memory-lifecycle, causal-reasoning, intent-routing |
| `core/competitors.py` | `get_agent_frameworks()` | Agent 框架特性矩阵 | session-lifecycle, memory-lifecycle |
| `core/competitors.py` | `get_ragas_baselines()` | RAGAS 生产基线 | memory-lifecycle, intent-routing |
| `core/competitors.py` | `get_cross_benchmarks()` | HotpotQA/AgentBench/BigBench-Hard | kg-reasoning, causal-reasoning |
| `core/reporter.py` | `Report` | 6 节报告渲染 | 全部 suite |

**新增 suite 指标（v50）**：
- `accuracy_pct` 扩展到 4 个 suite（原来仅 conversational-qa）
- RAGAS 评估集成到 conversational-qa（20 项随机样本）
- bleu1 计算 bug 修复（`len(raw)` → `len(bleus)`）
- raw results 新增 `"context"` 字段用于 RAGAS 评估

## 进程与脚本规范

- **禁止多次 `nohup` 无序启动**。使用 PID 变量 + `trap cleanup EXIT`。
- 启动前验证 model server 健康（`curl /models`），避免 plicod 启动后因 embedding 不可用而崩溃。
- plicod 每次 benchmark 前必须**全新启动**（`rm -rf ROOT`），消除状态污染。
- 脚本使用 `set -euo pipefail`，绝对路径解析（`SCRIPT_DIR` / `PROJECT_ROOT`）。
- Suite 失败时记录到 `FAILED_SUITES` 数组，不中断整体流程；运行后验证结果文件非空且含 `metrics`。
- `--dry-run` 预览配置；`--preprocess-timeout` 控制索引等待时间（默认 180s）。

## 环境变量

| 变量 | 说明 |
|------|------|
| `PLICO_HOST` / `PLICO_PORT` | plicod 地址 |
| `LLAMA_URL` | LLM server（默认 18920） |
| `EMBEDDING_API_BASE` | Embedding server |
| `LLM_BACKEND=openai` | 使用 OpenAI-compatible endpoint |
| `PLICO_KG_AUTO_EXTRACT=false` | Benchmark 时关闭 KG 提取以减少变量 |
| `PREPROCESS_TIMEOUT` | 索引等待秒数（脚本层） |

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

## 已测试的 Embedding 模型

| 模型 | 端口 | 维度 | 量化 | 搜索 hit_rate | 搜索延迟 |
|------|------|------|------|--------------|---------|
| Qwen3-Embedding-0.6B | 18921 | 1024 | Q8_0 | **85-90%** | **14ms** |
| Jina v5-small-retrieval | 18922 | 1024 | Q4_K_M | **0%** | **140ms** |

**关键发现**：
- Qwen3-Embedding-0.6B 是当前最佳选择
- Jina v5 GGUF 完全不可用（原因可能是 GGUF 转换问题或 pooling 策略不匹配）
- 切换模型只需改端口，plicod 重启即生效

## Benchmark Pipeline 经验

- **Search limit matters**: 5 → 15 snippets improved context hit rate significantly
- **Intent-specific prompts**: temporal/multi-hop questions need specialized prompts, not generic ones
- **F1 vs LLM Score**: F1 measures token overlap (low for paraphrased answers), LLM Score measures semantic correctness (better metric)
- **Context hit rate is the ceiling**: if search doesn't find the right content, no reader prompt can fix it
- **accuracy_pct 是对标标准**: LongMemEval/LoCoMo 竞争对手均使用 accuracy（score ≥ threshold），而非 F1/BLEU。4 个 suite 统一使用 `accuracy_pct()`
- **RAGAS 是 RAG 质量的正交维度**: accuracy_pct 测答案正确性，RAGAS 测答案忠实度/相关性/上下文质量。两者互补，不可替代
- **CID 优于关键词匹配**: 搜索结果用 CID 验证比 snippet 关键词匹配更可靠（proactive-optimization 已修复）
- **bleu1 分母陷阱**: `sum(bleus) / len(raw)` 会把 None 项拉低，必须用 `len(bleus)`（已修复）

## Benchmark Suite 矩阵（11 suites → 10 条公理全覆盖）

| Suite | 公理 | 关键指标 | 竞争对手基线 |
|-------|------|---------|-------------|
| conversational-qa | A2 | `accuracy_pct`, `ragas_faithfulness`, `ragas_answer_relevancy`, `ragas_context_precision`, `ragas_context_recall` | LongMemEval: Mem0 93.4%, OMEGA 95.4%, Mastra 94.87% / LoCoMo: EverMind 92.73%, Mem0 91.6%, Memori 81.95% / PersonaMem: Tencent 76.1% |
| retrieval | A6 | `recall@5`, `recall@10` | MTEB: Harrier #1, KaLM-12B #1, Qwen3-8B 70.58 |
| kg-reasoning | A8 | `avg_latency_ms`, `paths_found`, `path_validity_rate` | HotpotQA: Youtu-GraphRAG ~72%, IRRR 72.4% |
| performance | A5 | `p50_ms`, `p95_ms`（search/cas_write/recall/kg_path） | 自身历史基线 + Letta ~10ms recall |
| memory-lifecycle | A3, A9 | CRUD `success_rate`, `cross_layer_hit_rate`, `accuracy_pct`（layer migration）, `cp1_persistence_rate` | LongMemEval + LoCoMo + PersonaMem 全量竞争对手 |
| token-efficiency | A1 | `avg_tokens_per_query`, `cost_per_query_usd` | Memori 1294 tok, Zep 3911 tok, Mem0 1764 tok, TencentDB 61% reduction |
| scope-isolation | A4 | `leak_rate`（Private）, `cross_agent_access_rate`（Shared） | Agent 框架均无原生 scope 隔离 |
| session-lifecycle | A10 | `success_rate`, `search_persistence_rate` | LoCoMo 跨 session 持久化 + Agent 框架 session_mgmt 对比 |
| causal-reasoning | A8 | `bidirectional_rate`, `accuracy_pct`（causal retrieval） | LoCoMo temporal/multi-hop + HotpotQA + BigBench-Hard |
| intent-routing | A2 | `hit_rate` + `accuracy_pct` per intent type, `improvement_pct` vs no-intent | LoCoMo per-category + RAGAS Context Precision/Recall |
| proactive-optimization | A7 | L0/L1/L2 `avg_tokens_per_query`, `speedup_pct`, `search_recall_rate` | Token efficiency competitors + Mastra prompt-caching |

**accuracy_pct**：`core/metrics.py::accuracy_pct(scores, threshold=4)` — LLM-as-Judge 1-5 分制，score ≥ 4 = 正确。4 个 suite 使用（conversational-qa, memory-lifecycle, intent-routing, causal-reasoning）。

**RAGAS 评估**：`core/judge.py::Judge.evaluate_ragas()` — 4 个指标（faithfulness, answer_relevancy, context_precision, context_recall），0-10 整数归一化到 0.0-1.0。conversational-qa 在 20 项样本上运行。生产基线：Faithfulness 0.85+, Answer Relevancy 0.80+, Context Precision 0.65+, Context Recall 0.75+。

## 竞争对手基线（2026-05 更新）

硬编码在 `benchmarks/configs/competitor_baselines.yaml`，通过 `core/competitors.py` 加载。
每次 benchmark 运行自动渲染到报告中，包含：
- 分数对比表（含 notes、source、date）
- 架构分析摘要（每个竞争对手的关键技术和 Plico 可学习之处）
- Agent 框架特性对比矩阵（Memory Layers / Scope / KG / WASM 等）
- Key Learnings 可操作表（含优先级）

**竞争格局（2026-05 最新）**：
- LongMemEval: Supermemory ~99% (experimental), Mem0 93.4% (新算法), OMEGA 95.4%, Mastra 94.87%
- LoCoMo: EverMind HyperMem 92.73%, Mem0 91.6% (新算法, 从62.47%跃升), Memori 81.95%, Zep 79.09%
- PersonaMem: Tencent 76.1%, EverMind #2
- MTEB: Microsoft Harrier #1 multilingual, Tencent KaLM-12B #1 multilingual (2026-05)
- Agent 框架 (新增): TencentDB Agent Memory (L0-L3, 61% token reduction), agentmemory (6.2K stars, BM25+vector+KG), NevaMind memU (13.1K stars), EverMind/EverOS (ACL 2026, Skills Evolution Engine)
- 极端规模: BEAM benchmark (1M/10M tokens), MSA (100M token context, NeurIPS 2026)
- RAGAS 生产基线: Faithfulness 0.85+, Answer Relevancy 0.80+, Context Precision 0.65+, Context Recall 0.75+
- 跨领域参考: HotpotQA (multi-hop), AgentBench (agent capabilities), BigBench-Hard (reasoning), MemoryBench (latency+quality+cost)

**更新基线**：编辑 YAML 文件，格式见文件内注释。

## 报告格式（6 节）

1. **Summary** — 每 suite 一行：Key Metric / Value / Competitor Best / Gap
2. **Suite Results** — 每 suite 详细指标 + 竞争对手对比表 + 版本 delta
3. **Competitor Analysis** — 每个竞争对手的架构分析摘要
4. **Agent Framework Comparison** — LangChain/CrewAI/AutoGen/Letta/AIOS vs Plico 特性矩阵
5. **Soul Alignment Score** — 10 条公理每条 0-2 分，总计 /20
6. **Key Learnings** — 可操作的学习表（From Memory Specialists / Embedding Models / Agent Frameworks / RAGAS Targets / Cross-Benchmarks）

## Benchmark 数据

`benchmarks/results/` 下的 JSON 文件，版本号如 `v44`, `v48`, `v49`。

## 触发时机

| 场景 | 触发方式 | 依赖 |
|------|---------|------|
| **里程碑验收** | 必须运行 perf_regression 测试 | 无外部依赖（stub 后端） |
| **里程碑验收** | 建议运行端到端 benchmark suite | 需要 llama-server + plicod |
| **PR 合并前** | CI 自动运行 perf_regression | 无外部依赖 |
| **版本发布前** | 必须运行全量 benchmark + 生成报告 | 需要 llama-server + plicod |

### 无外部服务时（CI / 快速验证）

```bash
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression
```

结果直接输出到终端，无需存储文件。

### 有外部服务时（完整 benchmark）

```bash
cd benchmarks
PREPROCESS_TIMEOUT=600 ./scripts/run_full_benchmark.sh
```

结果存储：
- **JSON 结果**：`benchmarks/results/<suite>_<version>.json`
- **Markdown 报告**：`benchmarks/docs/benchmark_report_<version>.md`
- **里程碑快照**：`docs/milestones/vXX-summary.md` 中引用 benchmark 结果

## 报告存储规则

| 文件类型 | 路径 | 命名规则 |
|---------|------|---------|
| Perf regression 结果 | 终端输出 | 无需存储（CI 自动判断） |
| Benchmark JSON | `benchmarks/results/` | `<suite>_v<XX>.json` |
| Benchmark 报告 | `benchmarks/docs/` | `benchmark_report_v<XX>.md` |
| 里程碑快照 | `docs/milestones/` | `vXX-summary.md`（引用 benchmark 数据） |
