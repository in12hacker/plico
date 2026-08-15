# ADR-0001：个人数字分身与记忆原生数据模型

- 状态：Accepted
- 日期：2026-08-13

## 决策

Plico 面向单个个人用户，是用户的数字分身与外置认知系统，而不是企业多租户平台，也不是传统文件系统的 AI 搜索插件。

系统的长期主数据是可治理的记忆与证据：事实、经历、偏好、关系、过程、原始资料及其来源。向量、关键词索引、摘要、知识图谱和缓存都是可重建的派生数据。文档、表格、幻灯片、图表和图形界面是面向人的按需投影视图；它们可以导出或缓存，但不应成为 AI 读取信息的唯一主模型。

```text
输入资料/交互
      │
      ▼
原始证据（CAS） ──► 个人记忆（canonical）
                         │
               ┌─────────┼─────────┐
               ▼         ▼         ▼
            词法索引    向量索引    关系/时间视图
               └─────────┼─────────┘
                         ▼
                  按意图召回与组合
                         │
                 ┌───────┴────────┐
                 ▼                ▼
             AI 上下文       人类侧临时投影
                            文档/表格/PPT/图形
```

## 身份与数据边界

- 产品部署边界是个人设备或个人控制的单节点服务，优先 Embedded/UDS；远程监听属于显式开启的受保护能力。
- 不引入企业组织、企业租户、跨租户治理、组织级 RBAC、HA 控制面或计费隔离。
- 现有 `tenant_id` 仅作为兼容字段和个人本地命名空间保留，不能继续演化为企业租户模型。
- `agent_id` 表示数字分身内部的认知角色/执行主体，不等同于企业账户。远程请求仍必须证明身份，不能靠自报 `kernel`/`system` 获得危险能力。

## 存储与检索边界

- Memory 域以 `MemoryEntry::id` 为键，承载个人记忆生命周期、纠错、遗忘和访问轨迹。
- CAS 域以 CID 为键，承载不可变原始证据及可验证来源。
- 两个域可以通过显式引用关联，但不能直接混用 ID 或排名分数。Memory 候选不得使用以 CID 为键的 BM25 分数。
- 写入成功先保证 canonical memory；词法召回应立即可见；embedding 与其他昂贵索引允许最终一致，并必须可重试、可重建。
- 派生索引不得成为删除、纠错或恢复的唯一事实来源。

## 人类侧投影原则

后续文档/表格/PPT 生成应位于 projection/export 层，并满足：

1. 每个投影记录所用记忆、证据 CID、生成时间和模板/模型版本；
2. 默认可丢弃和重建，不反向覆盖 canonical memory；
3. 人类对投影的修订只有经过显式“确认/吸收”后，才形成新的记忆版本或证据；
4. AI 的日常召回应直接读取记忆与证据，而不是重复解析人为排版文件。

## 首轮实施与验收

本轮先建立最小正确闭环：

- Working Memory 写入不再被同步 embedding 阻塞；
- canonical 记录以 Pending 状态持久化，后台任务有界并发，失败或重启后可协调恢复；
- Pending 期间使用 Memory 同域词法召回；
- 晚到的索引任务不能复活已删除记录或覆盖已有向量；
- 性能报告分开记录 write acknowledgement、warm repeated search 和 cold unique search，不再对远程 embedding 的端到端请求套用统一 `<5ms` 门槛；
- 权限变更要求经认证的显式管理能力，危险远程操作不能信任调用者自报的系统身份。

## 后续顺序

1. 建立导入资料到“原始证据 + 规范化记忆 + provenance”的统一摄取协议；
2. 增加个人可检查、纠错、遗忘、合并和版本回溯的记忆治理 API；
3. 建立带 provenance 的 document/table/slides projection API；
4. 引入与 MemoryTier/MemoryType 正交的冷热投影状态机；
5. 用个人数字分身任务集验证长期记忆正确性、隐私边界和投影可追溯性。

## 冷热与记忆腐败

“腐败”只允许发生在检索投影，不能改坏或静默删除 canonical memory。三条轴必须正交：

- `MemoryTier` 表示认知职责（Ephemeral/Working/LongTerm/Procedural）；
- `MemoryType` 表示内容性质（Episodic/Semantic/Procedural/Untyped）；
- 后续 `RetrievalTemperature` 表示召回成本（Hot/Warm/Cold/Dormant）。

建议状态如下：Hot 常驻完整 embedding 与热 ANN；Warm 保留可快速装载的完整投影；Cold 使用独立 projection store，可保留量化向量、词法指纹和带来源版本的摘要；Dormant 只保留 canonical locator 与最小目录，deep recall 时重建投影。Cold/Dormant 不是第五个 MemoryTier，整个投影空间被删除后也必须能从 canonical memory/CAS 无损重建。

热度采用随时间衰减的分数、迟滞阈值和最短驻留时间。只有最终返回并被确认有用的结果才计命中；候选扫描不得改变热度。`importance`、TTL/删除和 temperature 分属价值、保留、召回成本三个维度，不能混用。Dormant 的一次 deep hit 先回到 Warm，连续命中或用户 pin 才进入 Hot。

召回协议逐层下探：`fast` 仅 Hot，默认 `balanced` 查询 Hot 后在覆盖不足时查 Warm，`deep` 可继续到 Cold 并为 Dormant 创建可观察的 rehydrate job。慢层耗时必须单独报告，不能与热层共用一个延迟门槛。

当前从 `embedding: None` 推导 Pending 是兼容过渡。实现冷热策略前，必须拆出 projection manifest，至少记录 canonical revision/content hash、模型与维度、状态（AbsentByPolicy/Queued/Building/Ready/Failed/Stale）和 temperature；否则有意卸载的 Dormant 投影会被后台协调器错误重建。

该迁移必须是单路径切换：新 manifest 的语义与旧快照路径经架构裁决后，一次性重写调用方并删除 `embedding=None` 状态推导、canonical 内嵌向量与相关兼容代码。禁止让两套状态源长期并存，也禁止为了保留旧签名在新 projection store 外再包适配层。

## 清债纪律

有调用证据的现行能力可以继续演进；经报告与调用图确认的死代码、假成功 mock、无调用示例二进制、失实注释和 marker 常量直接删除。库替换必须重写调用点后整体删除自制实现；如新库语义与旧实现不同，先做架构裁决，不能由兼容层静默吸收差异。

## 非目标

- 复刻桌面、目录树或 Office 文件编辑器；
- 用单个大模型替代用户做不可撤销决定；
- 企业多租户、团队知识库、组织合规或分布式高可用；
- 把知识图谱相关性描述成未经验证的因果推断。
