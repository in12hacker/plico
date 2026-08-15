# Plico 个人数字分身记忆评测与门禁规格

- 状态：Proposed
- 版本：0.1.0
- 日期：2026-08-13
- 架构依据：[ADR-0001](../../docs/adr/0001-personal-digital-twin.md)、[ADR-0002](../../docs/adr/0002-canonical-memory-and-reversible-retrieval-decay.md)

## 1. 目的与边界

本规格定义如何验证 Plico 的个人数字分身记忆，而不是给所有能力制造一个总分。它回答五个不同问题：

1. 长期交互中的事实能否被找回、更新并按时间回答？
2. 记忆机制是否支持准确检索、在线学习、长程理解和冲突处理？
3. 多年个人数字轨迹能否维持经历、情绪、观点和“不可回答”的边界？
4. 用户属性、习惯和偏好发生变化时，系统能否保留稳定事实并替换已变化状态？
5. 流式观察与用户反馈能否在未来相似任务中真正被复用？

本规格不测试企业知识库、多租户隔离、组织 RBAC、团队协作或 SaaS 计费。benchmark adapter 中出现的 `tenant_id` 只能固定为测试运行的本地命名空间，不得解释为产品支持企业租户。

## 2. 当前支持状态

截至本规格日期，仓库可见的 benchmark 路径只支持：

- `LongMemEval` 数据加载与 conversational-qa 适配；
- `MemoryAgentBench` 的 Accurate Retrieval（`memoryagentbench_ar`）切片；
- LoCoMo、BEIR 和已有合成/性能套件。

CloneMem、DynamicMem、MemoryAgentBench 的 TTL/LRU/CR、StreamMemBench，以及 ADR-0002 的 Hot/Warm/Cold/Dormant 验收，均是**计划适配或规格定义**，不是已实现能力。只有 adapter、数据 pin、结果校验和可复现实验都落地后，状态才能从 `unsupported` 改为 `research` 或 `official`。

## 3. 覆盖分层

五类外部 benchmark 是互补层，不是由低到高的排行榜，也不能把不同 judge、数据和 reader 模型的分数相加。

| 层 | Benchmark | 主要输入与时间跨度 | 主要能力 | Plico 架构问题 | 当前状态 |
|---|---|---|---|---|---|
| L1 长期对话记忆 | [LongMemEval](https://github.com/xiaowu0162/LongMemEval) | 带时间戳的多 session 对话；500 个问题 | 信息抽取、多 session 推理、知识更新、时间推理、拒答 | canonical 写入、session/turn 检索粒度、更新与 abstention | 已有适配；是否满足官方协议需按本规格重新认证 |
| L2 机制能力剖面 | [MemoryAgentBench](https://github.com/HUST-AI-HYZ/MemoryAgentBench) | 长文本被切块后增量多轮注入 | Accurate Retrieval、Test-Time Learning、Long-Range Understanding、Conflict Resolution | 不把“检索准”冒充完整记忆；检验在线学习与冲突链 | 仅 AR 切片可见；TTL/LRU/CR 未支持 |
| L3 数字分身纵向一致性 | [CloneMem](https://github.com/AvatarMemory/CloneMemBench) | 日记、社交、私信、邮件等 1–3 年非对话轨迹 | 事实、比较、轨迹、模式、因果、反事实、推断、不可回答 | 人生事件/情绪/观点的证据化建模；摘要是否损害 clone fidelity | 未支持 |
| L4 时变个人状态 | [DynamicMem](https://github.com/wenyaxie023/DynamicMem) | 15 个月、多应用活动；分 checkpoint 增量观察 | State Completion、Personalized Service；属性/习惯/偏好更新 | valid time、稳定与变化事实并存、当前状态重建、跨应用 provenance | 未支持 |
| L5 流式未来协助 | [StreamMemBench](https://github.com/landian60/StreamMemBench) | 时序多模态生活流；证据锚点上的 initial/follow-up 两步任务 | fidelity、initial evidence use、feedback incorporation、follow-up reuse | 观察→回答→反馈→修订→未来复用闭环 | 未支持；Plico 当前主要为文本路径 |

### 3.1 分层解释规则

- L1 通过不代表能维护完整个人状态；LongMemEval 以对话历史为主。
- L2 AR 通过不代表 TTL、LRU 或 CR 通过，报告必须逐能力分列。
- L3 高 QA 分不等于 provenance 正确；必须同时报告证据召回与不可回答。
- L4 必须按 checkpoint 报告，不得只给最终平均分掩盖随历史增长的退化。
- L5 必须保留 initial、revision、follow-up 的因果顺序；把反馈预先写入 initial context 属于泄漏。
- 外部 benchmark 不覆盖 canonical 哈希、冷热可逆性和投影重建，这些由第 6 节本地契约套件验证。

## 4. 公平基线

每个支持检索接口的数据集至少运行三种简单基线。所有基线使用相同 canonical 输入、时间截止点、query、top-k、reader、judge、token budget 和超时；差异只能来自被比较的记忆/检索表示。

### 4.1 BM25-only

- 索引单位固定为官方推荐粒度；若同时评估 turn/session，两者是两个独立 run；
- 只使用截止查询时已观察到的原始文本和允许的 metadata；
- 不使用 embedding、LLM 扩展、答案模板或测试问题生成的关键词；
- 记录 tokenizer、语言处理、BM25 参数、字段权重、top-k 与去重规则。

LongMemEval 官方仓库提供 `flat-bm25` 及 turn/session 粒度，可作为协议参照，但 Plico 的结果必须由自身 adapter 和固定 manifest 复现。

### 4.2 Vector-only

- 与 BM25 使用完全相同的原子单元和时间可见集合；
- 只允许一个固定 embedding 模型和一个明确的相似度；
- 记录模型 revision、维度、归一化、chunking、ANN 参数、是否 exact search；
- embedding 失败不能静默回退 BM25；该 run 应失败或把受影响样本标为 `infra_error`。

若向量为异步构建，正式 query 前必须满足可验证的 source watermark。固定等待秒数不构成“索引已就绪”的证据。

### 4.3 LLM-Wiki compiled projection

该基线验证“及时编译维护”是否优于简单检索，不把 Wiki 当成 Plico 主数据：

1. raw/canonical 输入保持不变；
2. 按时间顺序把已观察来源编译成带链接、来源 revision 和 evidence ID 的 Wiki 投影；
3. 固定 schema、compiler model、prompt、temperature、最大页面/token 预算与 lint 轮数；
4. compiler 在 query 前运行，严禁读取测试问题、参考答案或 future checkpoint；
5. query 只检索编译投影；若另加 canonical fallback，必须报告为单独的 `llm_wiki_plus_evidence` run；
6. 编译页不得引用自身作为 provenance，最终引用必须落到 benchmark evidence ID；
7. 同时报告 QA、evidence recall、编译 token/cost、更新延迟和 provenance coverage。

该基线源自 [Karpathy 的 LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)。[WiCER](https://arxiv.org/abs/2605.07068) 表明盲目 Wiki 编译可能因有损压缩丢失事实，所以必须保留无编译的 BM25/vector 基线，并将 query-informed refinement 限定为独立 research run，不能计入官方盲测。

### 4.4 可选 Plico hybrid

BM25 + vector + KG/reranker 是待比较系统，不是简单基线。报告必须展示它相对 BM25-only 和 vector-only 的增益、延迟与 token 成本，不能只展示最优结果。

## 5. 三类运行与门禁

| 类别 | 阻断对象 | 通过条件 |
|---|---|---|
| Official conformance | 阻断“官方可比/复现”声明与对应结果发布 | 上游协议、完整样本、数据 hash、judge/metric、manifest 与 failure ledger 全部合规 |
| Research evaluation | 不阻断合并；阻断把实验结果冒充官方或回归结论 | 清楚披露协议偏差、预算和不确定性，artifact 可回溯 |
| Regression gate | 阻断已实现能力的合并或发布 | 固定 fixture 的精确不变量及已基线化阈值全部通过 |

### 5.1 Official conformance

目标是产生可与 benchmark 官方协议比较的结果。要求：

- 固定官方仓库 commit/tag、原始数据 revision、split 和逐文件 SHA-256；
- 使用官方样本顺序、时间可见性、prompt、解析器、metric 和 judge 设置；
- 任何 adapter 转换都保留 stable sample ID，并有逐样本 source mapping；
- 完整运行官方要求的样本数；若因许可证或资源只能运行子集，结果必须标为 `research`；
- judge/model 与官方不一致时标记 `official-protocol-compatible`，不得声称复现官方分数；
- 结果校验、manifest 和 failure ledger 均通过才算有效。

Official conformance 是发布证据，但在 adapter 尚未实现时不阻断 Plico 代码发布；它不能被一个小型内部 fixture 代替。

### 5.2 Research evaluation

用于架构决策、模型比较与消融，包括：

- benchmark 子集、不同 reader/judge、query-aware 编译；
- Hot/Warm/Cold 策略、量化向量、摘要精度、深层检索预算；
- BM25/vector/hybrid/LLM-Wiki 比较；
- 成本、延迟、召回、provenance 和投影大小的 Pareto 分析。

Research run 可以失败或波动，不作为合并门禁。报告必须明确与 official protocol 的所有偏差，且不能把 research 分数写入官方对比表。

### 5.3 Regression gate

用于每次变更的确定性保护：

- 使用版本库内、许可允许的最小固定 fixture 或生成式合成数据；
- 固定随机种子和虚拟时钟；能用 stub/deterministic judge 的不调用远程模型；
- 只阻断已经实现并在 manifest 中声明 `supported` 的能力；
- 对 canonical/状态机不变量使用精确断言；对排序质量使用已基线化的阈值配置；
- 新 adapter 在进入 gate 前先积累稳定基线，不凭空设置“看起来合理”的数字。

门禁阈值必须存在版本化配置中，包含来源 run ID、样本数、置信区间或方差说明、批准人和生效版本。本文不虚构尚未测量的准确率/延迟目标。

## 6. 冷热腐败状态机验收

### 6.1 Fixture

最小 fixture 同时包含：

- 高频稳定事实、低频稳定事实和已经被纠正的旧事实；
- 跨多个时间点的偏好变化；
- 同义查询、精确关键词查询、时间查询和无答案查询；
- pinned、TTL、用户删除、冲突未决四种保留状态；
- Hot、Warm、Cold、Dormant 各层的预置样本；
- 每条记忆的 canonical hash、revision、evidence ID 和预期 epistemic state。

所有自动迁移使用可注入虚拟时钟。禁止依赖 wall-clock sleep。

### 6.2 必须通过的精确不变量

| ID | 操作 | 断言 |
|---|---|---|
| DECAY-01 | 记录 canonical snapshot，执行全层降温 | stable ID、revision、CID 与逐对象 SHA-256 100% 不变 |
| DECAY-02 | 删除所有派生 BM25/vector/KG/summary/Wiki 投影 | canonical 可读；状态为 Absent/Stale，而非 canonical missing |
| DECAY-03 | 从 canonical 全量重建投影 | source watermark 覆盖全部目标 revision；无 orphan 或 phantom record |
| DECAY-04 | 对每个 Cold/Dormant 样本执行 `deep` | 能返回同一 canonical revision，或显式返回可重试 rehydrate job；不得报告“确定不存在” |
| DECAY-05 | 只扫描候选但不采用答案 | verified-hit 计数与 temperature 不变 |
| DECAY-06 | 成功采用并确认 Cold/Dormant 结果 | 生成一条关联迁移事件；Dormant 首次回到 Warm，不越级自动 Hot |
| DECAY-07 | 重放同一迁移事件 | 幂等；无重复升温、重复计数或额外版本 |
| DECAY-08 | 改变 temperature | epistemic state、importance、valid time、provenance 与 TTL 不变 |
| DECAY-09 | 触发 pin、未决冲突、进行中任务依赖 | 策略按声明阻止降温；原因可审计 |
| DECAY-10 | TTL 到期或用户确认删除 | 使用独立 deletion/tombstone 事件；不得伪装成 Dormant |
| DECAY-11 | 导出并编辑 Markdown/PPT 投影 | canonical 不变；显式吸收后新增 evidence/revision |
| DECAY-12 | 注入构建失败、重启与迟到任务 | 失败不报告 Ready；迟到任务不能复活已删除记忆或覆盖新 revision |

### 6.3 分层质量与成本

在精确不变量通过后，research run 才测量：

- 每层 evidence recall@k、answer correctness、abstention；
- fast/balanced/deep 的 p50/p95/p99 latency；
- 每 query 读取字节、候选数、embedding/LLM token 与费用；
- rehydrate queue wait、build time、success/failure/retry；
- temperature 分布、迁移率、抖动率和 pin 比例；
- 相对“所有记忆保持 Hot”oracle 的 recall loss 与成本节省。

所有层共用一个延迟阈值是无效设计。Hot-path、逐层扩展和 rehydrate 分别建立基线；Cold 变慢是允许的，Cold 变得不可发现不是。

## 7. 数据、版本与哈希契约

每个 run 目录必须包含机器可读 `run_manifest.json`。至少记录：

```json
{
  "schema_version": "plico.memory-eval-run/v1",
  "run_id": "immutable-unique-id",
  "run_class": "official|official-protocol-compatible|research|regression",
  "benchmark": {
    "name": "longmemeval",
    "upstream_url": "https://github.com/xiaowu0162/LongMemEval",
    "upstream_commit": "full-commit-sha",
    "dataset_revision": "tag-or-full-commit-sha",
    "split": "longmemeval_s_cleaned",
    "license": "recorded-spdx-or-reference"
  },
  "artifacts": [
    {
      "logical_name": "raw-dataset",
      "path": "sanitized-relative-path",
      "bytes": 0,
      "sha256": "64-lowercase-hex"
    }
  ],
  "sampling": {
    "requested": 500,
    "actual": 500,
    "seed": 0,
    "sample_id_sha256": "64-lowercase-hex"
  },
  "pipeline": {
    "plico_git_commit": "full-commit-sha",
    "adapter_version": "semver-or-commit",
    "canonical_schema": "version",
    "projection_schema": "version",
    "source_watermark": "opaque-monotonic-value",
    "config_sha256": "64-lowercase-hex"
  },
  "models": {
    "embedding": {"provider": "name", "model": "id", "revision": "revision"},
    "compiler": {"provider": "name", "model": "id", "prompt_sha256": "sha256"},
    "reader": {"provider": "name", "model": "id", "prompt_sha256": "sha256"},
    "judge": {"provider": "name", "model": "id", "prompt_sha256": "sha256"}
  },
  "environment": {
    "started_at": "RFC3339",
    "hardware": "sanitized-description",
    "timeouts_ms": {},
    "endpoint_origins": ["scheme://host:port-without-secret-or-query"]
  }
}
```

补充规则：

- raw、规范化数据、split、sample-ID 列表、prompt、配置和最终结果分别计算 SHA-256；
- 远程数据只写 URL 不够，必须记录不可变 revision 与本地字节哈希；
- 数据清洗或字段转换会产生新 artifact，不能覆盖 raw；转换器版本和 parent hash 必须记录；
- 模型名不够，能获得 revision/digest 时必须记录；无法获得时明确写 `unavailable` 并降级可复现性声明；
- 不记录 API key、Authorization header、URL query secret、用户主目录或私有原文；
- 结果行必须携带 stable sample ID，聚合结果必须能回溯到纳入/排除的逐样本 ledger。

## 8. 失败协议

### 8.1 状态枚举

每个样本和整个 run 使用显式状态：

| 状态 | 含义 | 是否进入主指标分母 |
|---|---|---|
| `ok` | 输入、索引、预测和评分均完成 | 是 |
| `unsupported` | Plico adapter/能力未实现 | 否；run 不得标为通过 |
| `invalid_input` | 数据缺失、哈希不符、schema 或 stable ID 错误 | 否；official run 无效 |
| `index_not_ready` | source watermark 未达到 query 所需 revision | 否；run 失败，不得当作零召回 |
| `timeout` | 在声明的阶段超时 | 按官方协议；同时单独报告 |
| `infra_error` | 网络、模型、磁盘或依赖失败 | 否；run 不完整 |
| `judge_error` | judge 无响应、格式错误或无法解析 | 否；保留预测供重评 |
| `model_refusal` | 模型成功响应但拒绝任务 | 是，按官方协议计分 |
| `no_answer` | 系统明确判断证据不足 | 是，按 abstention 规则计分 |
| `partial` | 只完成请求样本的一部分 | 不得进入 official 表；可作 research 诊断 |

### 8.2 Fail-closed 规则

- 缺数据、哈希不符、索引未就绪、judge 失败时不生成成功指标；
- `0` 是有效测量值，`null/unavailable` 是没有测量，二者不得互换；
- 不允许从 vector 静默回退 BM25、从 remote 模型静默回退 stub，或自动缩小样本数；
- retries、最终异常、耗时和阶段写入 failure ledger；重试成功仍保留之前失败记录；
- requested/actual/scored/failed/excluded 样本数必须同时报告，并满足逐项和总数守恒；
- 聚合器遇到未知 metric、重复 sample ID、非有限数、越界值或 schema version 不兼容时退出失败；
- official run 的任何人工修补都产生新 run ID、manifest 和结果哈希，禁止覆盖原 artifact。

## 9. 泄漏与时序约束

- 任何 checkpoint 只能看到 `observed_at <= checkpoint` 的来源；晚到数据还需遵守 benchmark 明示的 ingestion 规则；
- compiler、summarizer、index expansion 与 reranker 不得读取 reference answer；
- 测试问题不能参与通用 memory construction；若实验需要 query-aware refinement，只能标为 research；
- StreamMemBench follow-up 前可以吸收该 item 的 feedback，但 initial prediction 前不能看到 feedback 或 follow-up；
- DynamicMem 每个 checkpoint 的状态独立评分，不能先构建最终 15 个月状态再回填早期答案；
- CloneMem 的反事实答案不能写回 canonical，unanswerable 不能被模型常识补全；
- LongMemEval abstention 样本没有 evidence location，不应混入官方 retrieval recall，但应保留在端到端 QA；
- MemoryAgentBench 各能力使用各自官方 metric，不把 substring exact match、exact match、Recall@5 和 LLM judge 分数合并。

## 10. 报告最小集合

有效报告至少包含：

1. run manifest 与所有 artifact hash；
2. run class、协议偏差和当前支持状态；
3. requested/actual/scored/failed/excluded；
4. 逐任务、逐 checkpoint、逐 temperature 的指标和分布；
5. BM25-only、vector-only、LLM-Wiki 与 Plico candidate 的同预算对照；
6. ingest、write acknowledgement、index readiness、query、judge 的分阶段延迟；
7. token、API 调用、读取字节、projection 大小和费用；
8. failure ledger、逐样本结果和可重跑命令；
9. 不能测量的项目及 `unsupported` 原因；
10. official protocol 的上游版本、许可和一手链接。

不得将 serial service rate 标成并发 QPS，不得将 warm repeated query 延迟标成 cold unique query，也不得把自制 RAGAS proxy 标成官方 RAGAS。

## 11. 引入顺序

1. 先为现有 LongMemEval 与 MemoryAgentBench AR adapter 补齐 manifest、哈希、failure ledger 和 official/research 标签；
2. 建立 DECAY-01 至 DECAY-12 的纯本地 regression fixture；
3. 实现 BM25-only、vector-only 与 LLM-Wiki compiled projection 的同预算 runner；
4. 接入 MemoryAgentBench TTL/LRU/CR，防止 AR 单项代表完整记忆；
5. 接入 CloneMem，建立数字分身长轨迹与 unanswerable 基线；
6. 接入 DynamicMem，验证 checkpoint 状态更新与跨应用 provenance；
7. 在多模态摄取边界明确后接入 StreamMemBench；文本转写子集只能标为 research；
8. 积累稳定运行后，将已实现能力的阈值写入版本化 regression 配置。

## 12. 一手资料

- [LongMemEval 官方仓库](https://github.com/xiaowu0162/LongMemEval)及[论文](https://arxiv.org/abs/2410.10813)
- [MemoryAgentBench 官方仓库](https://github.com/HUST-AI-HYZ/MemoryAgentBench)及[ICLR 2026 OpenReview](https://openreview.net/forum?id=DT7JyQC3MR)
- [CloneMem 官方仓库](https://github.com/AvatarMemory/CloneMemBench)及[论文](https://arxiv.org/abs/2601.07023)
- [DynamicMem 官方仓库](https://github.com/wenyaxie023/DynamicMem)及[论文](https://arxiv.org/abs/2606.22877)
- [StreamMemBench 官方仓库](https://github.com/landian60/StreamMemBench)及[论文](https://arxiv.org/abs/2606.14571)
- [Karpathy：LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
- [WiCER：Wiki 式记忆编译的评测与修复](https://arxiv.org/abs/2605.07068)
