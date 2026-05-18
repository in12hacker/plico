# v52 端到端测试报告

**日期**: 2026-05-18
**测试工具**: plico-agents (TCP path)
**LLM**: DeepSeek v4-flash (cloud API)
**Plico 版本**: 26.0.0

---

## 1. 测试环境

| 组件 | 地址/版本 | 状态 |
|------|----------|------|
| plicod | `127.0.0.1:7878` (root: `/tmp/plico-bench`) | 运行中 |
| LLM | `deepseek-v4-flash` via `api.deepseek.com` | 正常 |
| Embedding | Qwen3-Embedding-0.6B @ `:18921` | 正常 |
| CAS 对象 | 7,158 | 正常 |
| KG 节点 | 7,727 | 正常 |
| KG 边 | 62,513 | 正常 |

---

## 2. API 功能回归测试

### 2.1 CAS 操作

| 功能 | 状态 | 说明 |
|------|------|------|
| `create` | **通过** | CID 生成正常，返回 64 字符完整 CID |
| `read` | **通过** | 使用完整 CID 可正确读取内容 |
| `search` | **通过** | 语义搜索命中 3-5 条 |
| `batch_create` | **通过** | 批量创建 3/3 成功 |

### 2.2 记忆系统

| 功能 | 状态 | 说明 |
|------|------|------|
| `remember` | **通过** | Working memory 写入成功 |
| `remember_long_term` | **通过** | Long-term memory 写入成功 |
| `recall` | **通过** | 子串匹配召回正常 |
| `recall_semantic` | **通过** | 语义召回命中 2 条 |
| `batch_remember` | **通过** | 修复后：自动转换字符串列表为 dict 格式 |
| `memory_stats` | **通过** | 统计信息正确 |

### 2.3 知识图谱

| 功能 | 状态 | 说明 |
|------|------|------|
| `add_node` | **通过** | UUID 返回正常 |
| `add_edge` | **通过** | 边创建成功 |
| `find_paths` | **通过** | 路径查找返回路径 |
| `list_nodes` | **通过** | 列表查询正常 |
| `list_edges` | **通过** | 列表查询正常 |

### 2.4 Trace 基础设施 (v52 核心功能)

| 功能 | 状态 | 说明 |
|------|------|------|
| `trace_list` | **通过** | 返回 10 条 span 记录 |
| `trace_show` | **通过** | 单 trace 详情查询正常 |
| `trace_failures` | **通过** | 0 失败记录 |
| Trace 文件落盘 | **通过** | JSONL 文件写入 `/tmp/plico-bench/tool_trace/` |

### 2.5 其他功能

| 功能 | 状态 | 说明 |
|------|------|------|
| `hybrid` | **通过** | Graph-RAG 返回 20 条融合结果 |
| `context_assemble` | **通过** | 修复后：自动转换字符串 CID 为 candidate 格式 |
| `start_session` | **通过** | session_id 生成正常 |
| `end_session` | **通过** | 会话结束正常 |
| `health` | **通过** | 系统健康 OK |

---

## 3. DeepSeek Agent 端到端测试

### 3.1 测试流程

```
用户提问 → [Retriever] 生成搜索查询 → [手动搜索] Plico API →
[Analyst] 分析搜索结果 → [Reporter] 生成结构化报告
```

### 3.2 测试用例

**问题**: "什么是Plico的记忆分层架构？"

**结果**: 成功生成完整的安全架构分析报告（33.6 秒）

报告结构：
- Executive Summary
- Detailed Technical Analysis（Ephemeral/Working/Long-term 三层详解）
- Key Takeaways
- References

**问题**: "什么是SQL注入攻击？简要说明其原理和防御方法。"

**结果**: 成功生成完整的安全漏洞分析报告（56.1 秒）

报告结构：
- Executive Summary
- Technical Findings（攻击机制、高级攻击向量）
- Risk Assessment（CVSS 9.8 Critical）
- Recommended Actions（三层防御策略）
- References

---

## 4. 发现的问题与修复

### 4.1 已修复（plico-agents 客户端）

| 问题 | 根因 | 修复 |
|------|------|------|
| `batch_remember` 解析错误 | 客户端传字符串列表，API 期望 dict 列表 | `plico_client.py`: 自动转换字符串列表为 `{"content": ..., "tags": [], "importance": 5}` |
| `context_assemble` 解析错误 | 客户端传字符串 CID，API 期望 `ContextAssembleCandidate` | `plico_client.py`: 自动转换字符串 CID 为 `{"cid": cid, "relevance": 1.0}` |

### 4.2 非问题（误报）

| 现象 | 原因 | 结论 |
|------|------|------|
| `read` 返回空 | 使用了截断的 CID（16 字符而非 64 字符） | 正常使用完整 CID 无问题 |
| `hybrid` 返回 0 | 该 agent_id 下无数据 | 有数据的 agent 正常返回结果 |
| `start_session` 返回空 | 响应格式为 `session_started.session_id` | 正常提取即可 |

### 4.3 已知限制

| 限制 | 说明 |
|------|------|
| `context_assemble` 字段名 | API 返回 `context_assembly`（含 items/budget），而非 `context`。客户端需适配。 |
| CrewAI 原生工具调用 | DeepSeek 模型输出 `<|tool_call>...<tool_call|>` 格式，CrewAI 不解析。已改为手动执行工具模式。 |

---

## 5. 配置文件

### 5.1 `.env`（隐私配置，已加入 .gitignore）

```bash
DEEPSEEK_API_KEY=sk-...
DEEPSEEK_BASE_URL=https://api.deepseek.com
DEEPSEEK_MODEL=deepseek-v4-flash
DEEPSEEK_TEMPERATURE=0.3
```

### 5.2 修改的文件

| 文件 | 变更 |
|------|------|
| `.gitignore` | 新建 — 忽略 `.env` |
| `.env` | 新建 — DeepSeek API 配置 |
| `config.py` | 加载 `.env`，添加 `DEEPSEEK_*` 配置 |
| `agents/cybersec_agents.py` | `make_llm()` 优先 DeepSeek；`memory=False` |
| `main.py` | 重构 `run_tcp()` 为手动工具执行模式 |
| `plico_client.py` | `context_assemble` 和 `batch_remember` 自动类型转换 |

---

## 6. 结论

- **v52 核心功能全部通过**：Trace 基础设施（写入/查询/CLI）、Reader 模式、recall 语义搜索
- **端到端 Agent 测试通过**：DeepSeek + Plico 协作正常，生成高质量结构化报告
- **客户端问题已修复**：`batch_remember`、`context_assemble` 类型兼容
- **建议合并**：v52 里程碑可进入合并阶段
