# LLM Wiki 对抗性吸收笔记

调研对象是 Karpathy 提出的 LLM Wiki 模式及其可核验实现。这不是迁移方案，也不改变 Plico 的 AIOS/个人数字分身定位；只抽取可独立验证的维护机制，任何 Markdown Wiki 产品形态都不得直接进入 Plico 核心架构。

## 可吸收

- 三层责任分离：不可变 raw sources、AI 维护的 compiled knowledge、约束维护行为的 schema。Plico 对应为 CAS 原始证据、可版本化 canonical memory/knowledge projection、记忆治理策略。
- 新资料进入时增量整合，而不是每次问答重新解析全部原始文档：抽取实体/概念，更新已有综合，记录矛盾与链接。
- 内容哈希跳过未变化来源；持久摄取队列支持崩溃恢复、取消、有限重试和进度；完成后显式刷新索引版本。
- `purpose` 与 `schema` 分离：个人数字分身需要用户目标/关注方向作为召回与维护先验，但策略变更必须可审计。
- lint/review 回路：孤立知识、断链、缺来源、相互冲突、过时综合进入维护队列；不确定修改交给用户确认。
- 本地 loopback API + token、MCP 读取默认最小权限，以及 `fast/deep` 不同检索深度。

## 改造后吸收

- Wiki 页在 LLM Wiki 中是持久核心层；在 Plico 中只能作为 knowledge/document projection。它必须携带 source revision 和 provenance，可重建，不能覆盖原始证据或成为事实真值。
- “LLM 完全拥有 Wiki 层”改为“LLM 可提议维护，低风险派生投影自动更新；事实替换、遗忘、身份偏好等高影响变更走版本链或用户确认”。
- Markdown index/log 改为结构化 projection manifest + 追加事件日志；Markdown 仅是可阅读导出。
- 源文件 watcher 改为通用 Source Adapter：文件、消息、网页、传感输入进入同一 evidence ingestion protocol，而非把目录结构带进主数据模型。
- 页面级 lint 改为记忆级 maintenance：provenance coverage、冲突、陈旧投影、孤立实体、错误摘要、过期模型向量和冷热预算。

## 拒绝

- 把 Markdown 文件树、Obsidian/VS Code 浏览界面或 Office 格式作为 AI 的长期主存储。
- LLM 无条件覆盖已有知识页，或删除 source 后按文件归属级联删除所有综合内容。
- 每次 ingest 同时并发修改多个共享页面但缺少 revision compare、事务提交和冲突队列。
- 用漂亮的 Wiki 可读性替代可验证的 provenance、版本链、反事实纠错与删除语义。
- 企业 workspace/团队协作/多租户扩展；Plico 的边界是个人数字分身。
- 将 Plico 改造成 Wiki、笔记应用、文档管理器或 LLM Wiki 的兼容实现。

## 对 Plico 的维护闭环

1. `SourceObserved`：保存不可变证据，计算内容哈希，重复来源幂等跳过。
2. `MemoryCompilePlanned`：基于 purpose/schema 产出带 revision 的增量维护计划，列出 create/update/link/conflict/review。
3. `MemoryCommitted`：canonical memory 使用 append-only version/supersede 提交；高影响变更等待确认。
4. `ProjectionQueued`：embedding、词法、关系、摘要和人类文档投影独立异步构建。
5. `MaintenanceLinted`：周期检查断链、来源缺失、冲突、陈旧模型、失败投影和冷热预算。
6. `UserReviewed`：用户对争议事实、身份偏好和遗忘进行确认；反馈进入 purpose/schema 或新的记忆版本。

这套流程把 LLM Wiki 的“及时编译、持续维护”吸收进来，同时保留 Plico 的核心优势：AI 直接访问记忆与证据，人类文件只是临时可验证投影。

## 一手资料

- [Karpathy：LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f)
- [jackwener/llm-wiki：增量 ingest 协议](https://github.com/jackwener/llm-wiki/blob/main/skills/ingest/SKILL.md)
- [jackwener/llm-wiki：lint 协议](https://github.com/jackwener/llm-wiki/blob/main/skills/lint/SKILL.md)
- [jackwener/llm-wiki：基于内容哈希的增量同步](https://github.com/jackwener/llm-wiki/blob/main/src/commands/sync.ts)
- [LLM Wiki v2：记忆分层与保留设想](https://gist.github.com/rohitg00/2067ab416f7bbe447c1977edaaa681e2)
- [agentmemory：可审计的实现参照](https://github.com/rohitg00/agentmemory)
- [WiCER：wiki 压缩的信息损失与迭代修复](https://arxiv.org/abs/2605.07068)
