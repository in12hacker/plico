# Benchmark 与 plico-agents 安全/完整性审计（2026-08-13）

## 当前裁决

Plico benchmark 与 `plico-agents` 外部项目在该历史审计时共同使用唯一 `plico.personal.v1` 公共协议。外部面当时只有 13 项个人数字分身能力，不引入组织 tenant/RBAC，不保留旧 `ApiRequest` wire、generic MCP action 或 Python 兼容客户端。

## Benchmark 已闭环

1. Python client 严格校验 protocol、request ID、success/error 互斥与 data operation；business input 拒 agent/tenant/role/scope/auth。
2. TCP bearer 只由 transport 注入；异常不复制 bearer、peer message、details 或 provider 原错误。
3. mutating request 不重试；只读请求才允许重新连接。
4. 删除 KG reasoning、跨角色 scope isolation、伪 L0/L1/L2 token efficiency；无 suite alias。
5. object-storage、conversational QA、retrieval、session 与 performance 只调用真实公开能力。
6. performance 分报 readiness、object put/get/search、Working Memory canonical ack/get/lexical recall/index status/update/delete、session start/end；projection lag 轮询 typed status。
7. artifact 默认不含 raw results，以 0600 原子写；URL、环境与 cache metadata 脱敏并做 SHA-256 完整性校验。
8. 任一 suite、judge、artifact 或 transport 失败均 fail closed，不发布组合成功报告。

验证：benchmark `80 passed`；Ruff check/format、uv lock 与 shell syntax 全绿。

## plico-agents 已闭环

- 旧会重放写请求的 `plico_client.py` 已删除；新 `plico_api` 使用 strict DTO、4-byte frame 与 typed domain errors。
- write frame 发出后连接中断返回 `AmbiguousCommitError(request_id)`，不自动重放。
- dead CrewAI tools、generic `plico/plico_store`、KG/batch/hybrid/long-term/context/stats、字符串 grep 检查器和两份失败叙事 execution log 已物理删除。
- Markdown ingest 是逐项 canonical `object.put`；严格 UTF-8，只有 typed canonical success 才计成功。
- 三个 CrewAI 角色是同一 personal owner 内的认知分工，只接收 host 验证后的 object/memory hits；不持有 owner mutation tools。
- object 与 memory 证据分区提供，不混用 score；报告只做临时人类投影，不自动入库。
- Web UI 固定 `127.0.0.1`，没有 `--share`。
- trace 只记录 ID、operation、transport、phase、bytes、elapsed、count/path/degradation；sentinel 测试确认不记录 bearer 或正文。

验证：离线 `8 passed, 2 skipped`；隔离 daemon 13-op live loop `1 passed`；真实 `deepseek-v4-flash` Retriever→Analyst→Reporter grounding `1 passed`。真实模型报告引用集合是本轮 typed evidence IDs 的子集，workflow 未自动写 object/memory。

## 如实保留的风险

- 公开 object 是 immutable source；Markdown 内容变化会产生新 CID，v1 没有 object supersede/provenance，因此导入只能叫 append-only，不能叫及时同步。
- memory recall 目前是 lexical overlap；embedding Pending 不代表语义 memory recall 已实现。
- stub/远程 embedding 不可用时 object search 退化到 BM25、memory projection 保持 Pending；不能用真实 LLM 通过替代 retrieval-quality 结论。
- CrewAI 1.x 的真实调用产生 45 条依赖弃用警告；未隐藏，后续应在升级时清债。
- 内核仍有旧 semantic/internal handlers 与 Core variants，但已从 TCP/UDS/client/MCP/aicli 隔离；后续按生产调用图物理删除，不恢复 public 兼容层。
