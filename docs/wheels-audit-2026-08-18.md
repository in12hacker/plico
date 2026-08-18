# 造轮子审计 — 2026-08-18

全仓静态审计：凡手写实现与成熟生态方案重复之处，逐条标注位置、规模、测试证据与候选替代。
后续轮次按本文档逐项替换，**除非该项的鲁棒性已有可查证明**（T4 类附证明）。
审计方法：五路并行子代理扫全部模块 + 对关键指控逐条人工核实（MCP id 关联、intent 截断 panic、
cosine 重复、Ollama token 估算、usearch B1/Hamming 可用性均已亲验）。

背景：本仓已有去轮子先例 —— 自研 HNSW `edgevec` 已被 `usearch` 替换（Cargo.toml:37 注释），
`bm25`/`lru`/`redb` 均为真实依赖。本文档延续该方向。

**分级**
- **T1 立即替换**：成熟方案即插即用，且轮子自身有已核实的缺陷或零测试，鲁棒性无法证明
- **T2 评估后替换**：成熟方案明确，但有设计约束需先评估
- **T3 仓内去重**：不缺外部轮子，缺内部共享；无需新依赖
- **T4 保留**：领域正当或已有鲁棒性证明（附证据）
- **ARCH 架构级待决**：超出代码层替换权限，需 arch 决策

---

## T1 — 立即替换（缺陷在手，鲁棒性无法证明）

| ID | 轮子 | 位置 | 证据/缺陷 | 替代方案 |
|----|------|------|-----------|----------|
| W-01 | 手写 JSON-RPC 2.0 MCP 客户端（stdio 锁步假设） | `src/mcp/client.rs:51-220`（247 行） | **已核实缺陷**：发送 `"id"`（:64）但响应处理（:109/:177/:188）从不校验 `resp["id"]`，假设严格逐行锁步；服务器主动通知/进度消息即失步。传输层 0 单元测试（8 个测试全是对真实二进制的集成测试）；`tools/list`+`tests/mcp_test.rs` 里还有第三份重复的 JSON-RPC harness。能力协商形同虚设（发空 `{}` 且忽略返回）。 | 官方 `rmcp` crate |
| W-02 | 手写 JSON-RPC 2.0 MCP 服务器 + stdio 循环 | `src/bin/plico_mcp/rpc.rs:10-129`、`main.rs:108-134`（~200 行） | 错误码/请求校验/信封全部手写；spec 行为靠自觉（notification 丢弃这条做对了）。5 测试。 | `rmcp` server SDK |
| W-03 | 14 个工具的 JSON Schema 手写字符串字面量 | `src/bin/plico_mcp/tools.rs:13-235`（~220 行） | 与 `api/public/input.rs` 的 serde 类型双份维护，仅靠一个目录一致性测试约束。 | `schemars` derive |
| W-04 | ~40 个内置工具手写 schema 且**零校验** | `src/kernel/builtin_tools.rs:16-165+` | `execute_tool` 对入参不做任何 schema 校验（:207-227），schema 纯描述性 —— 既是轮子也是防线缺口。22 测试只测分发不测 schema。 | `schemars` + 校验 |
| W-05 | Ollama token 估算 chars/4，忽略真实用量 | `src/llm/ollama.rs:95-96`（已核实）；同法重复于 `kernel/ops/cost_ledger.rs:109`、`cognition/context_quality.rs:410-417`、benchmarks `metrics.py:143-150` | Ollama 响应含 `prompt_eval_count`/`eval_count`，代码已解析响应却弃之不用；中文内容 chars/4 偏差更大，直接污染成本账本。 | 读 provider usage 字段；或 `tiktoken-rs` |
| W-06 | intent 手写 `truncate` 字节切片 panic | `src/intent/heuristic.rs:602-608`（已核实） | `&s[..max_len]` 用于任意用户文本（:289/:311/:319/:406/:450）；≥17 个汉字即超 50 字节、切在 UTF-8 边界上直接 panic。`util::safe_truncate`（util.rs:47）已存在但此处未用。 | 改用仓内 `safe_truncate`（一行修复） |
| W-07 | 每次预取 spawn 线程 + 线程内新建一次性 Tokio runtime | `src/kernel/ops/prefetch.rs:640-646`；`PrefetchHandle`（:92-113）手写 promise（AtomicU8 状态机 + 轮询） | 全仓最典型的"重造执行器"：daemon 本有 runtime，却每次预取新建销毁一个；handle 是手写 oneshot。61 测试测的是业务不是这套执行机制。 | `tokio::spawn`/`spawn_blocking` + `tokio::sync::oneshot`（tokio 已是依赖） |
| W-08 | 5 个独立手写 CLI 参数解析器 | `aicli/input.rs:5-149`（完整 getopt 引擎）、`aicli/main.rs:81-149`、`plico_mcp/main.rs:73-87`、`plicod.rs:72-83,347-351`、`plico_memory_migrate/main.rs:86`（合计 ~390 行） | clap 已在 lockfile（criterion 传递依赖），体积论据不成立；aicli 仅 3 测试。 | `clap` derive（或 `pico-args`） |
| W-09 | 长度前缀帧协议实现两份 + 每请求新建连接 | `src/client.rs:252-284`（sync 版）、`src/bin/plicod.rs:394-430`（async 版）；`RemoteClient::send_request`（client.rs:131-171） | `MAX_FRAME_SIZE` 常量双份维护（client.rs:17 / plicod.rs:33）；客户端每请求新建 TCP/UDS 连接，无池化/重试/多路复用。 | `tokio-util` `LengthDelimitedCodec`；整栈可评估 `tonic`/`tarpc` |
| W-10 | embedding 逐调用 spawn 线程做 40s 超时，超时即泄漏线程 | `src/fs/embedding/mod.rs:49-65` | 每次调用一个 detached 线程 + mpsc + `recv_timeout`；超时路径 0 测试。 | `tokio::time::timeout`（需先异步化 provider，见 W-12） |

## T2 — 评估后替换（成熟方案明确，先过约束）

| ID | 轮子 | 位置 | 约束/现状 | 替代方案 |
|----|------|------|-----------|----------|
| W-11 | 两个分叉的手写 3 态熔断器 | `src/llm/circuit_breaker.rs`（128 行，4 测试）；`src/fs/embedding/circuit_breaker.rs:25-190`（~165 行，14 测试含并发与 trace 隐私） | embedding 版测试纪律好（部分证明）；但同一状态机在仓内两份且行为分叉才是真问题。 | `failsafe`；最低限度先合并为单实现 |
| W-12 | 手写 OpenAI 兼容 + Ollama HTTP 客户端，含 sync-over-async runtime 杂耍 | `src/llm/openai.rs:12-187`、`ollama.rs:8-128`（~300 行）；runtime 兜底在 openai.rs:162-167、ollama.rs:104-111 各一份 | 约束：`LlmProvider` trait 是同步的，替换需动 trait（波及 kg_builder 等阻塞消费方）。 | `async-openai`（base URL 可覆盖 DeepSeek/vLLM）、`ollama-rs` |
| W-13 | 手写配置三级级联 | `src/config.rs:295-498`（~300 行） | `merge_from`（:328-407）靠"与默认值比较"推断是否设置过 —— 无法区分"显式设为默认值"与"未设置"；默认值双声明（serde default fn + Default impl）需人工同步。 | `figment` / `config` |
| W-14 | 手写 1-bit 量化 + Hamming + 两阶段召回，**绕过** usearch | `src/fs/search/hnsw.rs:26-52, 205-246` | **已核实** vendored usearch-2.25.1 原生支持 `MetricKind::Hamming`（lib.rs:290）与 `ScalarKind::B1`（lib.rs:323）；现实现 ≥1000 条时对二值向量做 O(n) 全量线性扫 + 排序，用子线性索引换线性扫。另：持久化走 JSONL 全量 dump + 逐条重插重建（:298-395），而 usearch 有 `save/load/view`；头注释还声称 f16 实际用 I8（doc rot）。 | usearch 自身 B1+Hamming + `save/load` |
| W-15 | 手写邻接表图结构 + DFS/Dijkstra/PPR，"PetgraphBackend" 名不副实（仓内无 petgraph） | `src/fs/graph/backend.rs:31-60, 641-775, 1093-1166`（算法 ~230 行） | 图结构 RwLock<HashMap>；`find_weighted_path` 是手写堆式 best-first（且是**最大化**边权，语义需确认是否本意）；PPR 手写幂迭代。算法测试覆盖不错（tests.rs 44 个）但正确性证明弱于 petgraph 成熟实现。redb 持久化部分是好的，保留。 | `petgraph`（结构 + `algo::dijkstra/astar`）；PPR 可留在 petgraph 上自写 |
| W-16 | 手写句子/语义/Markdown 分块器 | `src/fs/chunking/mod.rs:80-282`（~230 行，11 测试） | 句子切分是无缩写处理的标点启发式；Markdown 结构靠手解析。约束：中文切分质量需与 `text-splitter`（SemanticSplitter + MarkdownSplitter/pulldown-cmark）实测对比。 | `text-splitter` |
| W-17 | 手写 CAS 引擎（分片目录 + temp/rename + 访问日志 LRU） | `src/cas/storage.rs`（410 行） | **写入无 `sync_all`、无目录 fsync**（:143-148）—— 与 ledger 路径的全套 fsync 编排不一致，崩溃可丢对象；LRU/淘汰（:254-282）靠 10 万条访问日志 + 懒持久化。6 测试，零崩溃/并发测试。 | `cacache`（自带完整性校验+LRU+durable 写）或 redb 表 |
| W-18 | 手写 daemon 生命周期（PID 文件 + SIGTERM+sleep(500ms)） | `src/bin/plicod.rs:92-191, 211-224`（~110 行） | stop 是竞态等待循环；PID/stop/status 0 测试。信号处理本身用 tokio::signal（对的）。 | systemd user unit + `sd_notify`（部署层）；或 `daemonize`/pidfd |
| W-19 | benchmarks 框架中与生态重复的 ~2,500 行 | `benchmarks/src/plico_benchmarks/core/llm.py`（1,065 行，手写重试/退避/429 分类 ≈ openai SDK + tenacity）；`metrics.py` 中 `ndcg_at_k`/`mrr`/`recall_at_k` 为死代码（实际用 ir-measures） | 证据完整性/预算/协议客户端（~12K 行）是领域代码，保留；只替换传输/重试层、删除死指标。 | `openai` SDK + `tenacity`；删死代码 |
| W-20 | 3 份复制的 CAS get 固定间隔重试 | `src/kernel/ops/cognitive_pipeline.rs:369-378, 398-406, 429-438`（标注 F-37） | 固定 100ms/200ms 无抖动；重试路径无直接测试。 | `backoff`/`tokio-retry`，或至少单 helper |
| W-21 | dispatch 500ms 轮询 + 双队列镜像 | `src/scheduler/dispatch.rs:34-35, 239-305`（代码 :243 自认"In a full implementation, this would use a shared channel"） | 每键 TokioMutex 串行化等设计是领域的；轮询底盘是轮子。 | `tokio::sync::mpsc`/`Notify` |
| W-22 | temporal 英文半区关键词表 | `src/temporal/rules.rs:150-253`（348 行实现，71 测试） | 20 条规则中 ~10 条英文（today/yesterday/last week…）恰是 `chrono-english` 全集；中文半区无 crate 可替（保留）。测试覆盖实打实，但 71 个里约 1/3 是琐碎断言。 | 英文半区委派 `chrono-english`；中文保留 |

## T3 — 仓内去重（无需新依赖）

| ID | 重复 | 位置 | 修复 |
|----|------|------|------|
| D-01 | `cosine_similarity` 实现 **6 份**（已核实） | `util.rs:33`、`fs/search/hnsw.rs:43`、`fs/search/memory.rs:28`、`kernel/ops/entity_resolver.rs:15`、`kernel/ops/conflict_detector.rs:15`、`kernel/ops/prefetch_cache.rs:167` | 归一到 `util` 单份 |
| D-02 | temporal 关键词表 **3 份**（互有出入） | `temporal/rules.rs:150`、`intent/heuristic.rs:552`（含 rules 解析不了的"上周末"）、`fs/query_augment.rs:164`（含解析不了的 "two weeks ago"） | 归一到 temporal 模块单源 |
| D-03 | LLM JSON 提取 2 个变体（一个切花括号、一个剥 markdown 围栏，互不覆盖对方场景） | `kernel/ops/kg_builder.rs:486-517`、`intent/llm.rs:80-96` | 单一共享 helper |
| D-04 | Mailbox 有界环用 `Vec::remove(0)` O(n) 淘汰 | `src/scheduler/messaging.rs:36-41` | `VecDeque`（语义是领域的，保留 bus 本体） |
| D-05 | `MAX_FRAME_SIZE`/`MAX_MESSAGE_SIZE` 常量双份 | `client.rs:17`、`plicod.rs:33` | 随 W-09 一并解决 |
| D-06 | 配置默认值双声明（serde default fn + Default impl） | `config.rs:187-291` | 随 W-13 一并解决 |
| D-07 | 两份近同的域分离 hash helper | `memory/ledger/hash.rs`、`memory/execution_observation/hash.rs` | 文件头声明"独立实现"是有意的防御性复制（WP1 政策）—— 标注为**有意保留**，不算事故 |

## T4 — 保留（领域正当 / 已有鲁棒性证明）

| 项 | 位置 | 证明 / 正当性 |
|----|------|----------------|
| bm25 封装 | `fs/search/bm25.rs`（235 行，13 测试） | 非轮子：对 `bm25` crate 的薄封装，仅加 max-score 归一化 |
| embedding OpenAI provider 重试 | `fs/embedding/openai.rs:45-68` | 非轮子：正确使用 `reqwest` 内建重试 |
| 混合 RRF 检索管线 | `fs/semantic_fs/mod.rs:895-1313` | 产品核心域逻辑，无 crate 对应；有融合/诊断/降级测试 |
| 权限护栏 PermissionGuard | `api/permission.rs`（16+24 测试） | 单进程小模型 + 9 种动作的自有语义；casbin 模型文件对此过重。自定义尾缀 glob 语义是已知折衷 |
| HMAC bearer 认证 | `api/agent_auth.rs`（14 测试） | 原语全部用对（hmac/sha2/rand/`subtle` 常时比较）；jsonwebtoken 是 JWT 形状，本地 daemon token 场景贴合度反而差 |
| `api/public` serde 线协议 | `input.rs`/`output.rs`（10 测试） | 惯用法：tagged enum + `deny_unknown_fields` + 有界校验 |
| CAS 安全加固层（0700/0600、NOFOLLOW、dev/ino 身份核验、有界对抗读） | `ledger_store.rs`、`projection_store.rs` 各处 | 无 crate 提供此能力；有 symlink/marker 对抗测试 |
| WP1 纯核心（JCS canonical、validation、ids、model） | `memory/execution_observation/*`（33 测试含 mutant-killing 反例测试） | JCS 来自 `serde_json_canonicalizer` 非手写；冻结上限是领域策略 |
| redb 图持久化 + 时序边键 + 迁移 | `fs/graph/backend.rs`（5 个 redb 回归测试，tests.rs:558-865） | 这本身就是"用了成熟方案"的正面样本 |
| EventBus | `kernel/event_bus.rs`（47 测试） | 扇出走 `tokio::sync::broadcast`；RingEventLog/JSONL 轮转是有重放语义的域逻辑 |
| 调度优先队列 | `scheduler/queue.rs` | std `BinaryHeap` 薄封装，Ord 正确 |
| InMemoryBackend 暴力检索 | `fs/search/memory.rs` | 文档明示的 MVP/测试替身（~40 个测试以它为 double） |
| intent 启发式路由主体 | `intent/heuristic.rs`（55 测试） | 目标类型是自有 `ApiRequest` 枚举，无 NLU crate 可映射；"关键词+置信度、LLM 兜底"是标准模式。W-06 的 panic 是其中唯一必须修的 |
| 中文 temporal 表 | `temporal/rules.rs` 中文条目 | 无 crate 覆盖 zh-CN；71 测试中日期断言部分扎实 |
| prompt 注册表 | `prompt/registry.rs`（17 测试） | 三级覆盖是域逻辑；`{{var}}` 渲染仅 15 行，替换 minijinja 收益低（注意其未声明变量静默不替换的失效模式） |

## ARCH — 架构级待决：三套并行手写持久化引擎（全仓最大轮子簇）

**范围**（按文件实数）：
- `src/cas/ledger_store.rs`（1,370 行）：flock + dev/ino 锁身份、双槽指针、`RENAME_EXCHANGE` 原子提交、`PublishedButUnsynced`、不可变内容寻址写、标记清扫 GC
- `src/cas/projection_store.rs`（5,022 行）：整树原子发布、两阶段 reset 标记、隔离区恢复、~30 个故障注入钩子
- `src/memory/ledger/store.rs`（1,748 行）：expected-head 乐观并发、hash 链根/段日志、**每次启动全链前缀重验**（O(代数×段数)）、写者中毒
- `src/memory/execution_observation/store/`（WP2，843 行代码）：双槽崩溃窗分类器（slots.rs 整个文件只因裸文件双槽发布存在）、全链验证 loader、不确定态 publisher、中毒 handle
- `src/cas/offline_migration.rs`（1,273 行，feature-gated）
- `src/cas/storage.rs`（410 行，已单列为 W-17）

**重叠分析**：redb **已是本仓依赖**且在图后端被验证过（含 10+ 回归测试）。上述引擎的事务基底——
双槽/交换/崩溃窗分类/中毒/启动重放——正是 redb 原生免费提供的（原子多对象提交、崩溃恢复、页面自校验）。
保守估计该基底占此簇代码 60-70%。

**不可替代的残余**（redb 不给、必须保留的能力，约占 30-40%）：
内容寻址 + 域分离哈希的**防篡改审计链**（B-tree 库无此物）、按对象类型的有界读上限、
0700/0600+NOFOLLOW 拓扑校验、fail-closed 不自动修复立场、capability 密封。

**鲁棒性现状（诚实评估）**：测试纪律存在——ledger 11 测、projection 49+2 测（含大量崩溃切点测试）、
memory/ledger 7+68 测、WP2 6 测（含两个 f06 崩溃窗测试）——但崩溃窗全部靠 `#[cfg(test)]`
注入标志模拟，**无真实 kill -9、无模糊测试、无性质测试**。据此不能宣称"已证明鲁棒"，
只能宣称"有测试纪律"。若继续保留裸文件路线，最低补强是：随机崩溃切点枚举测试（把注入点参数化遍历）。

**治理约束**：此簇设计由 ADR-0008 及 v53 WP 检查点冻结、经对抗评审，属架构决策；
替换 = 重开 ADR，不是代码层轮次。**本审计仅标注，不动结论。**

## 附带缺陷清单（审计中核实/高置信发现，替换之外需单独修）

| ID | 缺陷 | 位置 |
|----|------|------|
| B-01 | MCP 客户端不校验响应 id，锁步假设失步即错配 | `mcp/client.rs`（W-01 一并解决） |
| B-02 | `truncate` UTF-8 边界 panic | `intent/heuristic.rs:602`（W-06） |
| B-03 | token chars/4 估算污染成本账本 | `llm/ollama.rs:95` 等（W-05） |
| B-04 | 工具入参零校验 | `kernel/builtin_tools.rs`（W-04） |
| B-05 | CAS 对象写入无 fsync（与 ledger 路径不一致，崩溃可丢） | `cas/storage.rs:143-148`（W-17） |
| B-06 | hnsw 头注释声称 f16、实际 I8 | `fs/search/hnsw.rs:8-10`（W-14 顺带） |
| B-07 | `find_weighted_path` 最大化边权和 —— 语义是否本意需确认 | `fs/graph/backend.rs:689-775` |
| B-08 | 配置无法区分"显式设为默认值" | `config.rs:328-407`（W-13） |
| B-09 | embedding 超时线程泄漏 | `fs/embedding/mod.rs:49-65`（W-10） |
| B-10 | 文档化未实现：temporal ±7 天置信扩展；死代码 `expanded()`、`Granularity::HalfYear`、`util::safe_range` | `temporal/resolver.rs:26-36`、`rules.rs:24`、`util.rs:55` |
| B-11 | benchmarks 死指标 `ndcg_at_k`/`mrr`/`recall_at_k`（实际用 ir-measures） | `benchmarks/.../metrics.py` |
| B-12 | INDEX.md 陈旧：temporal/intent 描述不存在的 OllamaTemporalResolver；mcp INDEX 称用 tokio 实为同步 std | `temporal/INDEX.md`、`intent/INDEX.md`、`mcp/INDEX.md:29` |

## 建议替换顺序（供后续轮次）

1. **W-06 / W-05 / B-11 / B-12**：一行级修复与死代码清理，零风险热身
2. **W-07 / W-09 / D-01**：tokio 原语归位，删最多自定义并发代码
3. **W-01..W-04**：rmcp + schemars 一揽子，消除双份 schema 与协议缺陷
4. **W-08 / W-13**：clap + figment
5. **W-14 / W-15 / W-16**：检索与图（usearch B1、petgraph、text-splitter），各自需性能/质量对照实验
6. **W-11 / W-12 / W-10 / W-17 / W-18..W-22**：按约束逐个评估
7. **ARCH 项**：提交 arch 决策（redb 事务基底 vs 裸文件审计链），不在代码轮次内擅动

统计：T1 × 10、T2 × 12、T3 × 7（D-07 有意保留）、T4 × 15、ARCH × 1、缺陷 × 12。
