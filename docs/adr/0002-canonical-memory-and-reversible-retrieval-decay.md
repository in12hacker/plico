# ADR-0002：Canonical Memory、按需投影与可逆检索腐败

- 状态：Proposed
- 日期：2026-08-13
- 扩展：[ADR-0001：个人数字分身与记忆原生数据模型](./0001-personal-digital-twin.md)
- P3-A 细化：[ADR-0005：Memory Embedding Projection Manifest 单一控制面](./0005-memory-embedding-projection-manifest.md)
- 评测契约：[个人数字分身记忆评测与门禁规格](../../benchmarks/docs/personal-memory-evaluation-gates.md)

## 背景

Plico 的核心不是文件管理器，而是个人用户可验证、可纠错、可遗忘的数字分身记忆。AI 应直接消费结构化记忆和原始证据；文档、表格、幻灯片与 GUI 只在用户需要时生成。

长期运行会产生两个同时存在的压力：

1. 用户事实、偏好和习惯会变化，系统必须及时维护“现在有效的状态”，又不能抹掉过去发生过什么；
2. 记忆总量持续增长，不可能让所有 embedding、全文索引、摘要和图关系永久驻留在最快检索层。

因此，“知识更新”和“记忆腐败”必须是两个不同问题。前者改变当前解释，后者只改变检索成本。若把未命中直接等同于删除、把最新文本等同于真值，或让摘要覆盖证据，个人数字分身会在长时间运行后变得不可纠错。

## 当前实现边界

本 ADR 是目标架构，不代表以下状态机已经实现。当前仓库已有：

- CAS 的 SHA-256 内容寻址与不可变对象；
- Ephemeral / Working / LongTerm / Procedural 四种 `MemoryTier`；
- Working、LongTerm、Procedural 的持久化路径；
- `NotRequested / Pending / Ready` embedding 派生状态、词法召回和向量搜索；
- 可重建的搜索与图索引，以及事件记录基础。

当前尚不能据此声称已经具备：独立 projection manifest、Hot/Warm/Cold/Dormant 自动迁移、claim 级 provenance、基于确认命中的升温、跨全部派生索引的水位一致性，或 Dormant deep recall。实现这些能力前，相关 benchmark 必须报告 `unsupported`，不能用空值或成功退出伪装为通过。

## 决策

### 1. 产品边界是一个人的数字分身

Plico 的所有 canonical memory 都归属单个自然人及其本地数字分身：

- `tenant_id` 仅是兼容字段或个人控制域内的命名空间，不代表企业租户；
- `agent_id`、scope 或角色用于同一数字分身内部的认知职责与最小权限，不代表组织账户；
- 不增加企业组织树、跨租户共享、组织级 RBAC、租户计费、企业控制面或由平台运营者拥有的知识库语义；
- 远程接口仍需认证与授权，但安全边界是“个人所有者及其授权客户端”，不是 SaaS 租户模型。

外部协作者发送的内容可以成为来源，但不会因此获得 canonical memory 的共同所有权。分享或导出是受控投影，不改变主数据归属。

### 2. Canonical、派生索引与人类投影严格分层

```text
外部资料 / 交互 / 传感观察
              │
              ▼
      不可变证据（CAS CID）
              │ provenance
              ▼
  个人记忆版本 / claim 事件（canonical）
              │
       ┌──────┴──────────────┐
       ▼                     ▼
机器检索投影             人类侧投影
BM25 / ANN / KG          文档 / 表格 / PPT
摘要 / Wiki / 温度目录    图表 / GUI / 导出包
       │                     │
       └──────── 可丢弃、可重建 ┘
```

#### Canonical 数据

Canonical 层只包含可恢复事实历史所必需的数据：

- 原始证据内容及其 CID；
- 来源 locator、观察时间、有效时间和摄取时间；
- 记忆或 claim 的稳定 ID、版本、状态与证据引用；
- 用户纠错、合并、supersede、保留、删除与恢复事件；
- 为验证事件链所需的 revision、内容哈希和父版本引用。

事件应追加，不原地覆盖历史。所谓“当前事实”是对版本链和状态事件的可重建视图，不是唯一一份可变的 `latest.md`。

#### 派生机器投影

BM25、embedding/ANN、KG 边、摘要、LLM Wiki 页面、当前 claim 视图、热度分数和 tier residency 都是派生数据。每份投影至少绑定：

- canonical stable ID、revision 与 content hash；
- 投影类型、schema version；
- 构建器、模型、提示词或算法版本；
- 构建时间、source watermark；
- `Queued / Building / Ready / Failed / Stale / AbsentByPolicy` 状态。

派生投影可以删除、压缩、量化或异步重建，不能成为恢复证据或判定删除成功的唯一来源。

#### 人类侧投影

Markdown、文档、表格、幻灯片、图表和 GUI 是面向人的可读视图。投影可以缓存和导出，但必须：

1. 带出所用 memory revision、证据 CID、生成器版本与生成时间；
2. 默认只读回 canonical，不能通过保存文件静默覆盖记忆；
3. 用户修改只有经过显式“吸收/确认”操作，才形成新证据或新记忆版本；
4. 删除投影不删除 canonical，重建投影不改变事实置信度。

### 3. 三条状态轴彼此正交

同一条记忆至少有三类互不替代的状态：

| 状态轴 | 回答的问题 | 示例 | 禁止的混用 |
|---|---|---|---|
| `MemoryTier` | 它承担什么认知职责？ | Ephemeral / Working / LongTerm / Procedural | Cold 不是第五个认知层 |
| `RetrievalTemperature` | 找到它需要多少成本？ | Hot / Warm / Cold / Dormant | 不得从 temperature 推断真假或价值 |
| `EpistemicState` | 当前如何解释这条 claim？ | Asserted / Inferred / Contradicted / Superseded / UserCorrected | “最新”不自动等于“正确” |

`importance`、TTL/删除策略也分别表示价值先验和保留义务，不得复用为 temperature 或 epistemic state。

### 4. 记忆腐败是可逆的检索投影降级

“腐败”在 Plico 中专指派生检索能力随时间和预算逐层降级。它不能修改 canonical 字节、证据引用、历史版本或事实置信度。

| 温度 | 典型驻留 | 默认发现路径 | 允许的降级 |
|---|---|---|---|
| Hot | 完整 embedding、热 ANN、完整词法/metadata | `fast`、`balanced`、`deep` | 无损快速投影 |
| Warm | 可快速装载的完整投影或温索引 | `balanced`、`deep` | 移出热驻留，保留完整可加载投影 |
| Cold | 独立冷 projection store | `deep` 或覆盖不足时扩展 | 量化向量、词法指纹、带来源摘要、粗粒度图 |
| Dormant | canonical locator 与最小目录 | 显式 `deep`，创建 rehydrate job | 删除可重建投影；保留发现线索和 canonical |

建议状态机如下；阈值属于版本化策略，不写死在数据模型里：

```text
             verified useful hit / user pin
       ┌─────────────────────────────────────┐
       │                                     ▼
   Dormant ◄──── Cold ◄──── Warm ◄──── Hot
       │           │          │          │
       └─ rehydrate┴─ deep hit┴─ useful hit┘

向左：无确认命中 + 到达策略期限 + 满足最短驻留时间
向右：结果最终被使用并确认有用，或用户显式 pin
```

状态机必须满足：

- 只有最终返回并被用户、任务结果或显式反馈确认有用的结果才计命中；候选扫描、预取和 reranker 曝光不计命中；
- 使用迟滞阈值与最短驻留时间，避免边界附近来回抖动；
- 一次 Dormant deep hit 先 rehydrate 到 Warm；进入 Hot 需要连续有效命中或 pin；
- pin、法定/用户保留、进行中任务依赖和未解决冲突可以阻止降温，但不自动提高真值置信度；
- 所有迁移写入可审计事件，包含原因、策略版本、前后状态和 canonical revision；
- 任何温度层被清空后，均能从 canonical memory/CAS 重建；无法重建即说明派生层错误承载了主数据。

### 5. 召回按成本逐层扩展

召回协议提供明确预算，而不是一次查询所有层：

- `fast`：只查 Hot；适合交互式低延迟提示，允许显式返回 coverage 不足；
- `balanced`：先查 Hot，覆盖或置信不足时扩到 Warm；
- `deep`：依次查 Hot、Warm、Cold，并对 Dormant 创建有界、可取消的 rehydrate job；
- 用户要求“完整回忆”“寻找很久以前的资料”或 fast/balanced 明确不足时，可以升级深度；系统不得把 fast miss 表述为不存在。

Cold/Dormant 的最小目录必须允许独立于当前 embedding 模型的发现，例如稳定 ID、时间范围、来源类型、少量词法指纹与 canonical locator。否则 embedding 模型升级或冷投影删除会造成不可逆的“失忆”。

每层分别报告候选数、命中、延迟、读取字节和 rehydrate 状态。Hot 延迟不得掩盖 deep recall 成本，deep recall 也不得拖累 hot-path 门禁。

### 6. 及时维护采用证据驱动的版本链

摄取与更新使用以下目标闭环：

1. `SourceObserved`：保存不可变证据并计算内容哈希；同一来源 revision 幂等；
2. `ClaimProposed`：从证据提出结构化 claim，区分原文事实与模型推断；
3. `ReconciliationPlanned`：比较当前视图，列出 create、confirm、contradict、supersede、merge 和 review；
4. `MemoryCommitted`：追加新版本和证据边；高影响身份、偏好、遗忘与冲突解决等待用户确认；
5. `ProjectionQueued`：BM25、embedding、KG、摘要、Wiki 和人类投影独立构建；
6. `MaintenanceLinted`：检查缺来源、断链、相互冲突、过时模型、失败投影与冷层不可发现；
7. `UserReviewed`：用户确认成为新事件，不回写或篡改旧证据。

时间语义至少区分：

- `observed_at`：系统何时看到证据；
- `valid_from / valid_to`：事实在现实中何时有效；
- `ingested_at`：何时进入 Plico；
- `superseded_at`：何时被当前视图替换。

因此，“我换工作了”可以让当前职业 claim supersede 旧 claim，但旧职业在其有效区间仍可被时间查询召回。矛盾必须保留双方证据，在没有充分证据或用户确认前不得只因时间更晚而删除旧版本。

### 7. LLM Wiki 只作为编译投影吸收

Karpathy 的 [LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) 将 raw sources、LLM 维护的 Wiki 与 schema 分离，并提出 ingest/query/lint 循环。Plico 对其采用对抗性吸收：

| 决策 | 机制 | Plico 处理 |
|---|---|---|
| 吸收 | raw source 不可变、内容哈希增量摄取、purpose/schema、lint 队列 | 映射到 CAS、治理策略、投影水位和 maintenance event |
| 改造后吸收 | LLM 自动维护页面、链接与综合摘要 | Wiki 降级为可重建投影；事实更新走 claim 版本链和 provenance |
| 改造后吸收 | 查询结果可生成 Markdown、表格、幻灯片、图表 | 作为一次性人类投影，携带输入 revision 与证据 CID |
| 拒绝 | Markdown 文件树是知识真值 | AI 直接访问 canonical memory/evidence，文件不是核心协议 |
| 拒绝 | LLM 页面自引或综合文本自证 | provenance 必须终止于外部证据或用户确认，不能终止于模型输出 |
| 拒绝 | 自动硬删除低热度知识 | temperature 只删除可重建投影；canonical 删除需独立保留策略 |
| 拒绝 | 团队 Wiki、企业 workspace、多租户权限扩张 | 超出个人数字分身边界 |

[jackwener/llm-wiki ingest 协议](https://github.com/jackwener/llm-wiki/blob/main/skills/ingest/SKILL.md)、[lint 协议](https://github.com/jackwener/llm-wiki/blob/main/skills/lint/SKILL.md) 和[内容哈希增量同步](https://github.com/jackwener/llm-wiki/blob/main/src/commands/sync.ts)可作为维护流程参照，但其页面模型不进入 Plico canonical schema。

LLM 编译不是无损过程。[WiCER](https://arxiv.org/abs/2605.07068) 对 Wiki 式压缩的研究显示，盲目编译会丢失影响下游问答的事实；因此查询失败可以形成维护探针，但模型生成的补丁仍必须回到证据核验，不能根据测试答案修改 canonical。

## 验收不变量

实现本 ADR 时，下列项目是强制正确性条件；具体评测协议见配套 benchmark 规格：

1. **Canonical 不变**：仅做降温、升温、删索引与重建后，canonical 对象集合和逐对象 SHA-256 完全一致；
2. **可逆发现**：每个 Cold/Dormant fixture 都能通过 `deep` 找到并回溯到同一 canonical revision；
3. **状态可审计**：每次自动迁移有且只有一条可关联事件，失败不报告目标状态；
4. **命中无污染**：候选扫描、预取和失败回答不增加访问强化计数；
5. **事实与热度解耦**：温度变化不改变 epistemic state、importance、valid time 或 provenance；
6. **删除语义独立**：TTL、用户删除与合规擦除使用独立事件和策略，不伪装成 Dormant；
7. **投影可重建**：删除全部派生机器投影后，可从 canonical 重建到一致 source watermark；
8. **人类修改显式吸收**：编辑导出的文档不会静默改变 canonical；确认吸收会生成新证据/版本。

## 迁移约束

实现时引入唯一的 projection manifest 作为派生状态源。现有从 `embedding: None` 推导 Pending 的兼容逻辑、runtime 内嵌向量或重复状态源必须在一次迁移中被替换，不能长期双写。V1-B 后的 canonical ledger 已不含 embedding；P3-A 不迁移进程内向量，而是按 ADR-0005 从 verified canonical revision 全量分类并重建 artifact。

迁移顺序：

1. 定义 manifest 与状态事件 schema，并从 canonical revision 建立一次性全量分类/rebuild；
2. 让构建器、协调器和召回器只读写 manifest；
3. 增加 canonical 哈希审计和全量重建工具；
4. 完成 P3-A 门禁并删除旧状态推导和重复字段；P3-A manifest 不预留 temperature 字段；
5. 只有独立 DECAY/thermal ADR 接受后，才在后续 schema version 中用固定时钟 fixture 实现 temperature；
6. 通过 thermal 回归门禁后再启用真实自动降温。

不得用适配层长期维持两套状态真值，也不得在没有 deep discovery 路径时启用 Dormant。

## 后果

正面后果：

- 热检索可以保持小而快，冷记忆仍可恢复；
- 用户纠错和事实演化不再破坏历史；
- 文档/PPT/GUI 可按需变化而不迁移核心数据；
- embedding 模型、索引与 LLM Wiki 编译器可替换；
- benchmark 可以分别测量事实正确性、召回成本和维护闭环。

成本与风险：

- 需要显式版本链、projection manifest、水位与迁移审计；
- deep recall 延迟更高，必须异步、可取消并向调用方暴露；
- 自动 claim reconciliation 可能误判，必须保留证据并对高影响变更请求确认；
- 冷层摘要会有信息损失，最小目录和 canonical fallback 不可省略。

## 拒绝的替代方案

- **所有记忆永久驻留同一向量索引**：规模和模型迁移成本不可控，也无法解释召回预算；
- **按未命中天数硬删除**：把成本优化误当保留策略，破坏可逆性；
- **只保存最新摘要**：无法核验、纠错或回答历史时间问题；
- **把 Wiki/文档设为 canonical**：将人为排版和 LLM 压缩强加给 AI 主协议；
- **以相似度自动判定矛盾**：语义接近不等于逻辑冲突；
- **扩展企业多租户来解决 scope**：scope 是个人数字分身内部权限和认知隔离，不是产品边界扩张。

## 一手资料

- [LongMemEval 官方仓库与 ICLR 2025 协议](https://github.com/xiaowu0162/LongMemEval)
- [CloneMem 官方仓库：多年非对话数字轨迹与 AI Clone 任务](https://github.com/AvatarMemory/CloneMemBench)
- [DynamicMem 官方仓库：长期、多应用、时变个人状态](https://github.com/wenyaxie023/DynamicMem)
- [MemoryAgentBench 官方仓库：AR / TTL / LRU / CR](https://github.com/HUST-AI-HYZ/MemoryAgentBench)
- [StreamMemBench 官方仓库：流式证据、反馈与后续复用](https://github.com/landian60/StreamMemBench)
