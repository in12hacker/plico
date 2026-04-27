#!/usr/bin/env python3
"""Generate comprehensive benchmark report from all dimension results."""

import json
import time
from pathlib import Path

BENCH_ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = BENCH_ROOT / "results"
REPORT_DIR = Path(__file__).resolve().parent.parent.parent / "docs"


def load_json(path: Path) -> dict | None:
    if path.exists():
        with open(path) as f:
            return json.load(f)
    return None


def generate():
    REPORT_DIR.mkdir(parents=True, exist_ok=True)

    retrieval = load_json(RESULTS_DIR / "longmemeval_retrieval.json")
    e2e_ret = load_json(RESULTS_DIR / "longmemeval_e2e_retrieval.json")
    e2e_oracle = load_json(RESULTS_DIR / "longmemeval_e2e_oracle.json")
    mab = load_json(RESULTS_DIR / "memoryagentbench_ar.json")
    kg = load_json(RESULTS_DIR / "kg_multi_hop.json")
    perf = load_json(RESULTS_DIR / "perf_micro.json")

    lines = []
    lines.append("# Plico Benchmark 评测报告")
    lines.append("")
    lines.append(f"> 生成时间: {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append("> 评测系统: Plico (usearch HNSW cos f16 + all-MiniLM-L6-v2)")
    lines.append("")

    # Dimension 1
    lines.append("## 维度 1: 检索质量 — LongMemEval-S Retrieval")
    lines.append("")
    if retrieval:
        o = retrieval["overall"]
        lines.append(f"| 指标 | Plico | agentmemory BM25+Vec | MemPalace Vec |")
        lines.append(f"|------|-------|---------------------|---------------|")
        lines.append(f"| R@5 | **{o['recall@5']}%** | 95.2% | 96.6% |")
        lines.append(f"| R@10 | **{o['recall@10']}%** | 98.6% | ~97.6% |")
        lines.append(f"| R@20 | **{o['recall@20']}%** | 99.4% | — |")
        lines.append(f"| NDCG@10 | **{o['ndcg@10']}%** | 87.9% | — |")
        lines.append(f"| MRR | **{o['mrr']}%** | 88.2% | — |")
        lines.append("")
        lines.append("按题目类型:")
        lines.append("")
        lines.append("| 类型 | R@5 | R@10 | 数量 |")
        lines.append("|------|-----|------|------|")
        for qtype, m in retrieval.get("by_question_type", {}).items():
            lines.append(f"| {qtype} | {m['recall@5']}% | {m['recall@10']}% | {m['count']} |")
        lines.append("")
        lines.append(f"- 嵌入模型: {o.get('embedding_model', 'all-MiniLM-L6-v2')}")
        lines.append(f"- 索引配置: {o.get('index_config', 'usearch cos f16')}")
        lines.append(f"- 平均搜索延迟: {o.get('avg_search_time_ms', '?')}ms")
    else:
        lines.append("*未运行*")
    lines.append("")

    # Dimension 2
    lines.append("## 维度 2: 端到端记忆 — LongMemEval-S QA")
    lines.append("")
    for mode, data in [("retrieval", e2e_ret), ("oracle", e2e_oracle)]:
        if data:
            o = data["overall"]
            lines.append(f"### {mode.title()} 模式")
            lines.append(f"- 准确率: **{o['accuracy']}%** ({o['correct']}/{o['n_questions']})")
            lines.append(f"- LLM: {o.get('llm_model', '?')}")
            lines.append("")
            lines.append("| 类型 | 准确率 | 数量 |")
            lines.append("|------|--------|------|")
            for qtype, m in data.get("by_question_type", {}).items():
                lines.append(f"| {qtype} | {m['accuracy']}% | {m['count']} |")
            lines.append("")
    if not e2e_ret and not e2e_oracle:
        lines.append("*未运行（需要 LLM 服务）*")
    lines.append("")
    lines.append("对标: Zep=63.8%, Mem0=49%, Oracle-GPT4o=82.4%")
    lines.append("")

    # Dimension 3
    lines.append("## 维度 3: 增量记忆 — MemoryAgentBench AR")
    lines.append("")
    if mab:
        o = mab["overall"]
        lines.append(f"- 样本数: {o['n_samples']}")
        lines.append(f"- 命中率: **{o['hit_rate']}%**")
    else:
        lines.append("*未运行*")
    lines.append("")

    # Dimension 4
    lines.append("## 维度 4: 知识图谱 — KG 多跳")
    lines.append("")
    if kg:
        o = kg["overall"]
        lines.append(f"- 问题数: {o['n_questions']}")
        lines.append(f"- 实体存储: {o['total_entities_stored']}")
        lines.append(f"- 关系存储: {o['total_edges_stored']}")
        lines.append(f"- 路径命中率: **{o['path_hit_rate']}%** ({o['paths_found']}/{o['paths_attempted']})")
        lines.append(f"- LLM 抽取: {o.get('llm_extraction', False)}")
    else:
        lines.append("*未运行*")
    lines.append("")

    # Dimension 5
    lines.append("## 维度 5: 性能 — 微基准")
    lines.append("")
    if perf:
        lines.append("| 操作 | QPS | P50 (ms) | P95 (ms) | P99 (ms) |")
        lines.append("|------|-----|----------|----------|----------|")
        for r in perf.get("results", []):
            op = r["operation"]
            qps = r.get("qps", r.get("store_qps", "—"))
            p50 = r.get("latency_p50_ms", r.get("store_p50_ms", "—"))
            p95 = r.get("latency_p95_ms", r.get("store_p95_ms", "—"))
            p99 = r.get("latency_p99_ms", "—")
            lines.append(f"| {op} | {qps} | {p50} | {p95} | {p99} |")
    else:
        lines.append("*未运行*")
    lines.append("")

    # Summary
    lines.append("## 综合评估")
    lines.append("")
    lines.append("### 与竞品对比")
    lines.append("")
    lines.append("| 系统 | LongMemEval R@5 | 类型 | 特点 |")
    lines.append("|------|----------------|------|------|")
    r5 = retrieval["overall"]["recall@5"] if retrieval else "?"
    lines.append(f"| **Plico** | **{r5}%** | AI-OS Kernel (Rust) | 本地优先, CAS+KG+4层记忆 |")
    lines.append("| agentmemory | 95.2% | Memory Layer (TS) | BM25+Vector hybrid |")
    lines.append("| MemPalace | 96.6% | Vector Only | Pure vector search |")
    lines.append("| OMEGA | 95.4% (QA) | Memory Server (Python) | Local-first, SQLite |")
    lines.append("| Zep/Graphiti | 63.8% (QA) | Temporal KG (Python) | 时间推理 |")
    lines.append("| Mem0 | 49.0% (QA) | Cloud Memory (Python) | 即插即用 |")
    lines.append("")
    lines.append("### 瓶颈识别")
    lines.append("")
    if retrieval:
        o = retrieval["overall"]
        by_type = retrieval.get("by_question_type", {})
        weakest_type = min(by_type.items(), key=lambda x: x[1]["recall@5"]) if by_type else ("?", {"recall@5": 0})
        lines.append(f"1. **最弱题型**: {weakest_type[0]} (R@5={weakest_type[1]['recall@5']}%)")
        if o['recall@5'] < 95:
            lines.append("2. **检索质量**: 低于 agentmemory 基线，建议:")
            lines.append("   - 添加 BM25 混合检索")
            lines.append("   - 调整 usearch 参数 (M, ef_construction, ef_search)")
            lines.append("   - 尝试更高维度 embedding 模型")
    lines.append("")

    lines.append("### 改进路线")
    lines.append("")
    lines.append("1. **短期**: 调优 usearch 参数提升 recall")
    lines.append("2. **中期**: 添加 BM25 混合检索（仿 agentmemory）")
    lines.append("3. **长期**: 时间感知检索（仿 Zep/Graphiti temporal windows）")
    lines.append("")

    report_text = "\n".join(lines)

    report_path = REPORT_DIR / "benchmark-2026-04-25.md"
    with open(report_path, "w") as f:
        f.write(report_text)

    print(f"Report generated: {report_path}")
    print(f"  {len(lines)} lines")
    return report_path


if __name__ == "__main__":
    generate()
