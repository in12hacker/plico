#!/usr/bin/env python3
"""HotPotQA Multi-hop Reasoning Benchmark for Plico.

Evaluates Plico's KG multi-hop reasoning capability on HotPotQA dev distractor set.
Ingests supporting documents, uses KG paths + search to find answers.

Metrics: EM (Exact Match), F1, Supporting Fact F1.

Usage:
    python3 plico_hotpotqa_bench.py --data ../data/hotpotqa/hotpot_dev_distractor_v1.json
    python3 plico_hotpotqa_bench.py --data ... --sample 200 --reader-url http://127.0.0.1:18920/v1
"""

import argparse
import json
import os
import random
import re
import sys
import string
import time
from collections import Counter

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from plico_client import PlicoClient


def normalize_answer(s: str) -> str:
    """Lower text, remove punctuation, articles, and extra whitespace."""
    def remove_articles(text):
        return re.sub(r"\b(a|an|the)\b", " ", text)
    def white_space_fix(text):
        return " ".join(text.split())
    def remove_punc(text):
        exclude = set(string.punctuation)
        return "".join(ch for ch in text if ch not in exclude)
    return white_space_fix(remove_articles(remove_punc(s.lower())))


def exact_match_score(prediction: str, ground_truth: str) -> float:
    return 1.0 if normalize_answer(prediction) == normalize_answer(ground_truth) else 0.0


def f1_score(prediction: str, ground_truth: str) -> float:
    pred_tokens = normalize_answer(prediction).split()
    gold_tokens = normalize_answer(ground_truth).split()
    common = Counter(pred_tokens) & Counter(gold_tokens)
    num_same = sum(common.values())
    if num_same == 0:
        return 0.0
    precision = num_same / len(pred_tokens) if pred_tokens else 0
    recall = num_same / len(gold_tokens) if gold_tokens else 0
    return 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0


def call_llm(url: str, model: str, prompt: str, max_tokens: int = 128) -> str:
    """Call an OpenAI-compatible LLM."""
    import urllib.request
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": max_tokens,
        "temperature": 0.0,
    }).encode("utf-8")
    req = urllib.request.Request(
        f"{url}/chat/completions",
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            data = json.loads(resp.read())
            return data["choices"][0]["message"]["content"].strip()
    except Exception as e:
        return f"ERROR: {e}"


READER_PROMPT = """Answer the question based on the context. Give a short, direct answer.

Context:
{context}

Question: {question}

Answer:"""


def ingest_question_docs(client: PlicoClient, item: dict, q_idx: int) -> str:
    """Ingest supporting + distractor documents for one HotPotQA question."""
    agent_id = f"hotpot-{q_idx}"
    context = item.get("context", [])
    for doc_idx, (title, sentences) in enumerate(context):
        text = f"{title}: {' '.join(sentences)}"
        client.create(
            content=text[:2000],
            tags=[f"hotpot:q{q_idx}", f"doc:{doc_idx}", f"title:{title}"],
            agent_id=agent_id,
        )
    return agent_id


def evaluate_question(
    client: PlicoClient, item: dict, q_idx: int,
    reader_url: str, reader_model: str,
) -> dict:
    """Evaluate one HotPotQA question."""
    agent_id = ingest_question_docs(client, item, q_idx)
    time.sleep(0.3)

    question = item["question"]
    gold_answer = item["answer"]
    level = item.get("level", "unknown")
    qtype = item.get("type", "unknown")

    resp = client.search(
        query=question,
        agent_id=agent_id,
        limit=5,
        require_tags=[f"hotpot:q{q_idx}"],
    )
    snippets = [r.get("snippet", "") for r in resp.get("results", []) if r.get("snippet")]
    context = "\n".join(snippets)

    prompt = READER_PROMPT.format(context=context, question=question)
    answer = call_llm(reader_url, reader_model, prompt)

    em = exact_match_score(answer, gold_answer)
    f1 = f1_score(answer, gold_answer)

    sp_facts = item.get("supporting_facts", [])
    sp_titles = set(t for t, _ in sp_facts)
    retrieved_titles = set()
    for r in resp.get("results", []):
        for tag in r.get("tags", []):
            if tag.startswith("title:"):
                retrieved_titles.add(tag[6:])
    sp_precision = len(sp_titles & retrieved_titles) / len(retrieved_titles) if retrieved_titles else 0
    sp_recall = len(sp_titles & retrieved_titles) / len(sp_titles) if sp_titles else 0
    sp_f1 = 2 * sp_precision * sp_recall / (sp_precision + sp_recall) if (sp_precision + sp_recall) > 0 else 0

    return {
        "q_idx": q_idx,
        "question": question,
        "gold_answer": gold_answer,
        "predicted": answer,
        "em": em,
        "f1": f1,
        "sp_f1": sp_f1,
        "level": level,
        "type": qtype,
        "num_context_docs": len(snippets),
    }


def main():
    parser = argparse.ArgumentParser(description="HotPotQA Benchmark for Plico")
    parser.add_argument("--data", default=os.path.join(
        os.path.dirname(__file__), "..", "data", "hotpotqa", "hotpot_dev_distractor_v1.json"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7878)
    parser.add_argument("--reader-url", default="http://127.0.0.1:18920/v1")
    parser.add_argument("--reader-model", default="qwen2.5-7b-instruct")
    parser.add_argument("--sample", type=int, default=200, help="Number of questions to sample")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--output", default=None)
    args = parser.parse_args()

    with open(args.data, "r", encoding="utf-8") as f:
        all_items = json.load(f)
    print(f"Loaded {len(all_items)} questions from HotPotQA dev distractor")

    random.seed(args.seed)
    items = random.sample(all_items, min(args.sample, len(all_items)))
    print(f"Sampled {len(items)} questions for evaluation")

    client = PlicoClient(host=args.host, port=args.port)
    results = []

    for i, item in enumerate(items):
        print(f"[{i+1}/{len(items)}] Q{i}: {item['question'][:60]}...", end=" ", flush=True)
        try:
            r = evaluate_question(client, item, i, args.reader_url, args.reader_model)
            results.append(r)
            print(f"EM={r['em']:.0f} F1={r['f1']:.2f} SP={r['sp_f1']:.2f}")
        except Exception as e:
            print(f"ERROR: {e}")
        finally:
            client.close()

    print(f"\n{'='*60}")
    print("HotPotQA Results")
    print(f"{'='*60}")

    from collections import defaultdict
    by_type = defaultdict(list)
    for r in results:
        by_type[r["type"]].append(r)
        by_type["overall"].append(r)

    summary = {}
    for qtype, items_list in sorted(by_type.items()):
        avg_em = sum(r["em"] for r in items_list) / len(items_list)
        avg_f1 = sum(r["f1"] for r in items_list) / len(items_list)
        avg_sp = sum(r["sp_f1"] for r in items_list) / len(items_list)
        summary[qtype] = {
            "count": len(items_list),
            "em": avg_em,
            "f1": avg_f1,
            "sp_f1": avg_sp,
        }
        print(f"  {qtype:15s}  n={len(items_list):3d}  EM={avg_em:.3f}  F1={avg_f1:.3f}  SP_F1={avg_sp:.3f}")

    output_path = args.output or os.path.join(os.path.dirname(__file__), "hotpotqa_results.json")
    with open(output_path, "w") as f:
        json.dump({"summary": summary, "details": results}, f, indent=2, ensure_ascii=False)
    print(f"\nResults saved to {output_path}")


if __name__ == "__main__":
    main()
