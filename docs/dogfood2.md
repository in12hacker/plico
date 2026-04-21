dogfood Trial Round 2 — 完整报告
测试矩阵
37 项 CLI/tool 能力 逐项测试，覆盖 CAS、Memory、KG、Agent、Session、Events、Tools、Hybrid、Intent。

正常的部分
功能	状态
CAS put/get/search/search+tag/update
全部正常
Memory remember/recall (跨 agent)
正常
KG node/edge/nodes/paths/explore
正常
Agent register/agents list/discover
正常
Session session-start (compound response) + delta
正常
Events history (结构化输出)
正常
Tool list (37 tools) / tool call
正常返回 JSON
JSON 输出模式 (AICLI_OUTPUT=json)
比 human-readable 更丰富
Intent submit
正常
发现的 12 个 Bug (B9–B20)
Critical
Bug	描述	影响
B9
search 返回已软删除的对象 — CAS delete 成功（回收站确认），但 search 仍返回该 CID
删除后数据泄漏，违反软删除契约
B10
hybrid search 永远返回 0 结果 — 任何 query/seed-tags/depth 组合都是 "0 items, 0 vector, 0 graph"
Graph-RAG 管道在 stub embedding 下完全不可用
High
Bug	描述	影响
B11
CLI delete 无权限时静默失败 — exit 0，无错误消息。需要先 permission.grant delete，但 CLI 不告诉你
AI agent 以为删除成功了实际没有
B12
Agent 生命周期操作不持久化 — suspend/resume/terminate 返回 exit 0 但无效果，agents 列表始终显示 "Created"
Agent 状态管理完全失效
Medium
Bug	描述
B13
context.load L0 不降级 — 请求 L0 返回 layer="L2" 全文内容，L0/L1 分层没生效
B14
events history --agent 不过滤 agent_id — --agent plico-dev 仍返回 cli 的事件
B15
10 个 CLI 命令完全无输出 — delete/restore/suspend/resume/terminate/session-end/remember/edge/status/quota 全部 exit 0 但不打印任何内容
Low
Bug	描述
B16
tool describe 只显示名称+描述，不显示参数 schema
B17
explore 显示 relatedto 而非 related_to（缺下划线）
B18
get 不存在的 CID 返回 exit 0 无错误消息
B19
tool call 不存在的 tool 返回空无错误消息
B20
growth 显示 Sessions: 0（刚 session-start 过）
核心结论
数据层 (CAS/Memory/KG) 是稳固的 — 基本 CRUD 和搜索可靠
协调层 (Agent lifecycle / Hybrid / Permissions) 问题严重 — 生命周期不持久化、hybrid 完全失效
静默失败是 #1 DX 问题 — 10 个命令无输出，AI agent 无法区分成功和失败
JSON 输出模式是亮点 — 包含 snippet、created_at、token_estimate，比 human-readable 模式实用得多