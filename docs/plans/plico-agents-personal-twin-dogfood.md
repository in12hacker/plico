# plico-agents 个人数字分身 Dogfood 契约

`plico-agents` 外部项目是个人数字分身的网络安全 dogfood，不是第二套 API，也不形成组织多租户产品面。它曾完成 personal.v1 dogfood；Plico 已破坏式切到 `plico.personal.v2`，因此外部 demo 在完成 exact-14 迁移前属于明确阻断的旧消费者。本计划不授权在 Plico 仓内保留 v1 reader、alias 或 adapter。

## 已冻结的闭环

```text
Markdown 输入适配
  → object.put canonical source
用户问题
  → session.start
  → host 执行 object.search 与 memory.recall
  → Retriever / Analyst / Reporter 只消费 verified hits
  → 临时 Markdown / CLI / loopback Web 投影
  → session.end
用户显式确认
  → memory.create / update / delete
```

三个 Agent 是一个 personal-owner vault 内的认知分工，不是身份、租户或 memory namespace。Owner tools 不交给 LLM；报告不自动写回 canonical memory。

## 单轨实现裁决

- `plico_api` 必须仅声明 personal.v2 exact-14 operations；删除 `memory.index_status`，新增
  `projection.status` 与 owner-only `projection.rebuild`，input DTO 拒绝 unknown/agent/tenant/role/scope/auth 字段。
- UDS 无 payload auth；TCP 只由 transport 注入 bearer。
- mutating request 不自动重试。写帧后连接中断返回 `AmbiguousCommitError(request_id)`。
- object 与 memory hit 保持两个分数空间，分区交给模型，不手写融合分数。
- import 逐项 `object.put`，只认 typed canonical success；严格 UTF-8。
- UI 只绑定 `127.0.0.1`，无 public share。
- trace 不记录 bearer、问题、正文、snippet、prompt、tags、完整路径或 provider 原错误。

旧 `plico_client.py`、generic MCP/CrewAI tools、KG/batch/hybrid/long-term/context/stats 调用、字符串 grep “Soul 检查器”、成功叙事 execution logs 与 requirements 双轨已物理删除。

## 如实声明的缺口

- 公开 memory recall 当前只有 lexical overlap；manifest Ready 也不等于语义 recall 已启用。
- Markdown 导入是 append-only，不是及时同步。相同内容由 CID 去重，修改内容生成新 CID；当前公共面尚无 object supersede/provenance，因此旧版本可能继续被搜到。
- 报告 grounding 的结构门禁只验证引用 ID 来自本轮 typed hits；真实答案质量需要本地/远程 LLM 独立评测。
- thermal/deep recall 与 human-document projection 仍是后续 ADR，不通过 demo 适配层提前伪造。

## 验收门槛

1. personal.v2 exact-14 DTO/catalog 与 identity-field rejection；personal.v1 和 `memory.index_status` 均 fail closed。
2. 截断/超限/错 request ID/错 operation/domain error fail-closed。
3. 响应丢失时 mutating request 只发送一次。
4. object put/get/search、memory create/get/recall/update/delete、projection status/rebuild、session watermark 的 live loop 与 restart durability。
5. embedding identity/worker 不可用时，control-plane、worker、status observation 分层报告；不得压成 Pending/Ready。
6. workflow 异常仍执行 durable session.end；报告不触发 memory write。
7. trace sentinel 检查确保 bearer、问题、正文与 provider raw error 不出现。
8. `pytest`、Ruff、uv lock 全绿；真实 LLM/embedding E2E 单独报结果。

## 2026-08-13 历史 personal.v1 验收记录

- 隔离 stub daemon 的 13-op UDS live loop：1/1 通过；object search 如实报告 BM25 + `provider_unavailable`，memory projection 如实保持 Pending。
- 离线 protocol/workflow/import/projection：8 通过，live/real 两项默认显式 skip。
- `.env` 中真实 `deepseek-v4-flash` grounding E2E：1/1 通过；报告只引用本轮 object CID/MemoryEntry ID，且 workflow 没有自动 canonical 写入。
- 首次真实模型调用暴露 DeepSeek 当前不支持 CrewAI `output_pydantic` 使用的 `response_format`；已删除该非必要依赖，Retriever 改为单条 plain-text query，不增加模型专属兼容层。
- CrewAI 1.x 在真实 E2E 发出 45 条自身 deprecated-field 警告；不影响本轮结果，但作为依赖升级债务保留，未用 warning filter 隐藏。

上述记录是 Historical evidence，不能作为 personal.v2 发布门禁。B2 必须生成新的 exact-14、七类写操作单帧不重试、projection manifest status/rebuild、restart 与真实 LLM 绑定 artifact。
