# v53 W0 开发组交付说明

- 基线：`f0c745f`（branch `v53-w0-hygiene`；任务单 tag `v53-w0-handoff` = `7148f4d`）
- 任务单：`docs/milestones/v53-w0-developer-handoff.md`（唯一 scope 依据）
- 本文件：交付说明 + Architecture Deviation 申报（任务单 §2 允许 `docs/milestones/v53-w0-*.md`）

## Architecture Deviation（一处，请求事后追认）

**新增 `benchmarks/tests/test_metrics_estimation.py`（4 个 pytest）**。
任务单 §1 W-05 明令"benchmarks 站点……补定向测试/断言（benchmarks 走其自身 pytest
门）"，但 §2 允许清单对 benchmarks 仅开 `benchmarks/**/metrics.py`（限 metrics 模块）。
pytest 的收集约定（`tests/test_*.py`）使定向测试无法落在 metrics 模块内；测试文件为
纯新增、零生产代码影响。请架构组在轻量 review 中追认或剔除。

## 已知限制（非本单范围，建议下轮）

- `src/temporal/mod.rs:18` 仍残留 ±7 天失实表格行（mod.rs 不在允许清单，未动）；
  temporal 各文件头部的 "LLM-based resolver" 措辞与 INDEX 中 "match arms in resolver"、
  置信分层描述亦为既有失实（消费者不读 confidence），均超出 B-12 "恰 2 处" 边界。
- benchmarks 环境：`test_benchmark_p0.py::test_dead_python_embedding_stack_is_removed`
  在本机失败——`uv run` 的 editable 构建会在 pytest 收集前再生成被测断言要求不存在的
  egg-info 目录（git-ignored）；干净基线复现同样失败，与本候选无关（已做 stash 归因）。
- W-05：embedding/CAS 路径（cost_ledger、context_quality）无 provider usage 字段可读，
  保持注释声明的 chars/4 回退；真实修复需上游提供用量，超出 W0。
- ollama `prompt_eval_count=Some(0)` 会如实记录 0（服务器上报值，按"读真实字段"语义
  不加 max(1)）。

## 门禁证据摘要（原始输出见交付报告）

- 定向：intent_ 133/0、temporal_ 21/0、ollama 15/0、cost_ledger 9/0、context_quality 27/0
- benchmarks：293 passed / 1 failed（上述环境项，基线同败）
- fmt / `git diff --check` / clippy `-D warnings`：全净
- 全库 `--lib --all-features` 恰一次：**2150 passed / 0 failed / 2 ignored**（账目：
  基线 2146 − 删除 2 个 expanded 死代码测试 + 新增 6 测试）
- 死代码零残留 grep：`.expanded(` 0；`HalfYear` 0；`safe_range` 0（仅
  `jcs_safe_range` 测试名含子串）；本地 `fn truncate` 0；三死指标 0
