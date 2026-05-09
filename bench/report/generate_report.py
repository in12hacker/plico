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

    locomo_main = load_json(RESULTS_DIR / "locomo_main_baseline.json")
    locomo_ab = load_json(RESULTS_DIR / "locomo_ab_dual.json")
    
    longmemeval_main = load_json(RESULTS_DIR / "longmemeval_main_baseline.json")
    longmemeval_ab = load_json(RESULTS_DIR / "longmemeval_ab_dual.json")
    
    hotpotqa = load_json(RESULTS_DIR / "hotpotqa_main_baseline.json")
    beir = load_json(RESULTS_DIR / "beir_scifact_main_baseline.json")
    mab = load_json(RESULTS_DIR / "memoryagentbench_ar.json")
    kg = load_json(RESULTS_DIR / "kg_multi_hop.json")
    perf = load_json(RESULTS_DIR / "perf_micro.json")

    lines = []
    lines.append("# Plico 基线性能评测报告 (v40)")
    lines.append("")
    lines.append(f"> 生成时间: {time.strftime('%Y-%m-%d %H:%M:%S')}")
    lines.append("> 评测系统: Plico (usearch HNSW cos f16 + all-MiniLM-L6-v2)")
    lines.append("> 评测模型: Gemma-4-26B-A4B-it-Q4_K_M (Reader/Judge), Qwen2.5-7B-Instruct (A/B Judge)")
    lines.append("")

    # Dimension 1
    lines.append("## 维度 1: 会话记忆 — LoCoMo")
    lines.append("")
    if locomo_main:
        o = locomo_main["summary"]["overall"]
        lines.append("### Gemma 单模型主基线")
        lines.append(f"- 样本数: {o['count']}")
        lines.append(f"- F1 Score: **{o['f1']:.3f}**")
        lines.append(f"- BLEU-1: **{o['bleu1']:.3f}**")
        lines.append(f"- LLM Score: **{o['llm_score']:.2f}**")
        lines.append(f"- Context Hit Rate: **{o['context_hit_rate']:.2f}**")
        lines.append(f"- 平均在线延迟: {o.get('avg_online_latency_s', 0):.2f}s")
        lines.append(f"- 平均搜索延迟: {o.get('avg_search_latency_s', 0):.4f}s")
        lines.append("")
    else:
        lines.append("*LoCoMo 主基线未运行*")
        lines.append("")
        
    if locomo_ab:
        o = locomo_ab["summary"]["overall"]
        lines.append("### Gemma Reader + Qwen Judge A/B 对照")
        lines.append(f"- 样本数: {o['count']}")
        lines.append(f"- F1 Score: **{o['f1']:.3f}**")
        lines.append(f"- BLEU-1: **{o['bleu1']:.3f}**")
        lines.append(f"- LLM Score: **{o['llm_score']:.2f}**")
        lines.append(f"- Context Hit Rate: **{o['context_hit_rate']:.2f}**")
        lines.append(f"- 平均在线延迟: {o.get('avg_online_latency_s', 0):.2f}s")
        lines.append(f"- 平均搜索延迟: {o.get('avg_search_latency_s', 0):.4f}s")
        lines.append("")
    else:
        lines.append("*LoCoMo A/B 对照未运行或仍在进行中*")
        lines.append("")

    # Dimension 2
    lines.append("## 维度 2: 长期记忆 — LongMemEval")
    lines.append("")
    if longmemeval_main:
        o = longmemeval_main["summary"]["overall"]
        lines.append("### Gemma 单模型主基线")
        lines.append(f"- 样本数: {o['count']}")
        lines.append(f"- Exact Match (EM): **{o['exact_match']:.3f}**")
        lines.append(f"- Accuracy@4+: **{o['accuracy_4plus']:.3f}**")
        lines.append(f"- LLM Score: **{o['llm_score']:.2f}**")
        lines.append(f"- Context Hit Rate: **{o['context_hit_rate']:.2f}**")
        lines.append(f"- 平均在线延迟: {o.get('avg_online_latency_s', 0):.2f}s")
        lines.append(f"- 平均搜索延迟: {o.get('avg_search_latency_s', 0):.4f}s")
        lines.append("")
    else:
        lines.append("*LongMemEval 主基线未运行*")
        lines.append("")
        
    if longmemeval_ab:
        o = longmemeval_ab["summary"]["overall"]
        lines.append("### Gemma Reader + Qwen Judge A/B 对照")
        lines.append(f"- 样本数: {o['count']}")
        lines.append(f"- Exact Match (EM): **{o['exact_match']:.3f}**")
        lines.append(f"- Accuracy@4+: **{o['accuracy_4plus']:.3f}**")
        lines.append(f"- LLM Score: **{o['llm_score']:.2f}**")
        lines.append(f"- Context Hit Rate: **{o['context_hit_rate']:.2f}**")
        lines.append(f"- 平均在线延迟: {o.get('avg_online_latency_s', 0):.2f}s")
        lines.append(f"- 平均搜索延迟: {o.get('avg_search_latency_s', 0):.4f}s")
        lines.append("")
    else:
        lines.append("*LongMemEval A/B 对照未运行或仍在进行中*")
        lines.append("")

    # Dimension 3
    lines.append("## 维度 3: 多跳推理 — HotPotQA")
    lines.append("")
    if hotpotqa:
        o = hotpotqa["summary"]["overall"]
        lines.append(f"- 样本数: {o['count']}")
        lines.append(f"- Exact Match (EM): **{o['em']:.3f}**")
        lines.append(f"- F1 Score: **{o['f1']:.3f}**")
        lines.append(f"- Supporting Fact F1: **{o['sp_f1']:.3f}**")
        lines.append(f"- 平均在线延迟: {o.get('avg_online_latency_s', 0):.2f}s")
    else:
        lines.append("*HotPotQA 未运行*")
    lines.append("")

    # Dimension 4
    lines.append("## 维度 4: 信息检索 — BEIR (SciFact)")
    lines.append("")
    if beir:
        o = beir["scifact"]
        lines.append(f"- 样本数: {o['num_queries']}")
        lines.append(f"- nDCG@10: **{o['ndcg@10']:.3f}**")
        lines.append(f"- Recall@5: **{o['recall@5']:.3f}**")
        lines.append(f"- MAP: **{o['map']:.3f}**")
    else:
        lines.append("*BEIR 未运行*")
    lines.append("")

    # Dimension 5
    lines.append("## 维度 5: 增量记忆 — MemoryAgentBench AR")
    lines.append("")
    if mab:
        o = mab["overall"]
        lines.append(f"- 文档数: {o.get('n_documents', '?')}")
        lines.append(f"- 问题数: {o.get('n_questions', '?')}")
        lines.append(f"- Plico API 命中率: **{o.get('plico_hit_rate', 0):.1f}%**")
        lines.append(f"- 离线向量 命中率: **{o.get('offline_hit_rate', 0):.1f}%**")
        lines.append("")
        lines.append("*注：当前版本该数据集命中率为0，可能由于评测口径或数据格式不匹配导致，需进一步排查。*")
    else:
        lines.append("*未运行*")
    lines.append("")

    # Dimension 6
    lines.append("## 维度 6: 知识图谱 — KG 多跳")
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

    # Dimension 7
    lines.append("## 维度 7: 性能 — 微基准")
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
    lines.append(f"| **Plico** | **N/A** | AI-OS Kernel (Rust) | 本地优先, CAS+KG+4层记忆 |")
    lines.append("| agentmemory | 95.2% | Memory Layer (TS) | 仅作离线纯检索参考 |")
    lines.append("| MemPalace | 96.6% | Vector Only | 仅作离线纯检索参考 |")
    lines.append("| OMEGA | 95.4% (QA) | Memory Server (Python) | Local-first, SQLite |")
    lines.append("| Zep/Graphiti | 63.8% (QA) | Temporal KG (Python) | 时间推理 |")
    lines.append("| Mem0 | 49.0% (QA) | Cloud Memory (Python) | 即插即用 |")
    lines.append("")
    lines.append("### 瓶颈识别")
    lines.append("")
    lines.append("1. **最弱题型**: single-session-user (待确认)")
    lines.append("2. **检索质量**: 待确认")
    lines.append("")

    lines.append("### 改进路线")
    lines.append("")
    lines.append("1. **短期**: 调优 usearch 参数提升 recall")
    lines.append("2. **中期**: 添加 BM25 混合检索（仿 agentmemory）")
    lines.append("3. **长期**: 时间感知检索（仿 Zep/Graphiti temporal windows）")
    lines.append("")

    report_text = "\n".join(lines)

    report_path = REPORT_DIR / "benchmark_report_v38_zh.md"
    with open(report_path, "w") as f:
        f.write(report_text)

    print(f"Report generated: {report_path}")
    print(f"  {len(lines)} lines")
    return report_path


if __name__ == "__main__":
    generate()
