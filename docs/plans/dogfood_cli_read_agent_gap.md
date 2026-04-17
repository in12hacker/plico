# Dogfood 检索闭环缺口：`get` 与对象所有权（调查 + 修复计划）

> **Tier B** — 记录一次真实调查结论，避免只存在于对话里。  
> **后续里程碑**：非默认 agent 下 **完整语义 CRUD + SKILL 对齐** 已写入 [v0.6_dogfood_closing_exec_fs.md](v0.6_dogfood_closing_exec_fs.md)（下一迭代执行）。  
> **现象（历史）**：按 `plico-dogfood` skill 把计划/ADR 写入 `PLICO_ROOT` 后，用 `search` 能列出 CID，但用 `get <CID>` 读正文失败。

---

## 1. 调查结论（根因）

| 能力 | 本地 `aicli` 行为 | 结果 |
|------|-------------------|------|
| `search` | `cmd_search` 使用 `extract_arg(..., "--agent")`，默认 `cli`，可传 `plico-dev` | 与 dogfood 写入身份一致时，**能查到** CID |
| `get` / `read` | `cmd_read` 调用 `kernel.get_object(&cid, "cli")`，**忽略 `--agent`** | 对象 `meta.created_by == plico-dev` 时，权限拒绝：**无法读取** |

**代码位置**（修复前）：

```371:373:src/bin/aicli.rs
fn cmd_read(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = args.get(1).cloned().unwrap_or_default();
    match kernel.get_object(&cid, "cli") {
```

对比：TCP 路径 `build_request` 里 `ApiRequest::Read` **已**带上 `agent_id`（`--agent`），因此 **仅本地直连内核模式** 与 daemon 模式不一致。

**语义**：不是「dogfood 存坏了」或「搜索坏了」，而是 **CLI 本地 read 路径未把调用者 agent 传给内核**，与所有权模型冲突。

---

## 2. 影响范围

- **plico-dogfood** skill 推荐的 `cargo run ... aicli get` 在默认 `plico-dev` 写入场景下**不可靠**。
- 临时绕过：用脚本直接读 `objects/<2位前缀>/<余下>` JSON 再解码 `data` 字节数组（维护成本高）。
- **不影响**：TCP 客户端若正确传 `Read.agent_id`；`put`/`search` 已带 `--agent` 的本地路径。

---

## 3. 修复计划（最小改动）

1. **本地 `cmd_read`**：`agent_id = extract_arg(args, "--agent").unwrap_or("cli")`，调用 `kernel.get_object(&cid, &agent_id)`。
2. **`print_help`**：在 `get/read` 小节补充 `--agent ID` 说明（与 `put`/`search` 一致）。
3. **可选**：在 `plico-dogfood` SKILL 的「读取示例」中写明：`get <CID> --agent plico-dev`（修复后）。

### 验收

- `EMBEDDING_BACKEND=stub aicli --root /tmp/plico-dogfood get <cid> --agent plico-dev` 能打印正文（CID 来自同根下 `search --require-tags plico:milestone:v0.5 --agent plico-dev`）。
- 不传 `--agent` 时行为与现网一致（默认 `cli`）。
- 现有 `tests/cli_test.rs` 仍通过。

### 非目标（本计划不展开）

- 向量 stub 下「纯语义」检索弱：属 embedding 后端策略，与所有权 bug 独立。
- 为 `get` 增加 `--cid` 与位置参数双形态：可另开 UX 小单。

---

## 4. 状态

| 项 | 状态 |
|----|------|
| 调查结论写入本文件 | 已完成 |
| `aicli` `cmd_read` 使用 `--agent` | **已完成**（与仓库 `src/bin/aicli.rs` 一致） |
| SKILL `plico-dogfood` 示例更新 | 列入 **v0.6** [v0.6_dogfood_closing_exec_fs.md](v0.6_dogfood_closing_exec_fs.md) Task B |
| `plico-dev` 全链 put/search/get/update/delete 自动化 | 列入 **v0.6** Task A |
