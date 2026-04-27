#!/usr/bin/env python3
"""LongMemEval QA Benchmark for Plico — long-term memory evaluation.

Evaluates Plico on LongMemEval dataset: ingest sessions, search, answer, judge.

Categories: SS-User, SS-Assistant, SS-Preference, K-Update, Temporal, Multi-Session.

Usage:
    python3 plico_longmemeval_qa.py --data ../data/longmemeval_s_cleaned.json
"""

import argparse
import json
import os
import re
import sys
import time
from collections import defaultdict

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from plico_client import PlicoClient

READER_PROMPT = """Based on the following conversation history, answer the question concisely.
If you don't have enough information, say "I don't know."

Context:
{context}

Question: {question}

Answer:"""

JUDGE_PROMPT = """Rate the answer accuracy from 1-5 compared to the ground truth.
5=perfect, 4=mostly correct, 3=partial, 2=mostly wrong, 1=completely wrong.

Question: {question}
Ground Truth: {gold}
Prediction: {answer}

Output ONLY the number 1-5:"""


def load_longmemeval(path: str) -> list[dict]:
    """Load LongMemEval dataset."""
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)
    if isinstance(data, dict):
        return list(data.values())
    return data


def call_llm(url: str, model: str, prompt: str, max_tokens: int = 256) -> str:
    """Call an OpenAI-compatible LLM endpoint."""
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
            result = json.loads(resp.read())
            return result["choices"][0]["message"]["content"].strip()
    except Exception as e:
        return f"ERROR: {e}"


def ingest_sessions(client: PlicoClient, item: dict, item_idx: int) -> str:
    """Ingest all sessions for one LongMemEval item."""
    agent_id = f"lme-{item_idx}"
    sessions = item.get("sessions", item.get("conversations", []))
    for si, session in enumerate(sessions):
        turns = session if isinstance(session, list) else session.get("turns", session.get("messages", []))
        for ti, turn in enumerate(turns):
            if isinstance(turn, str):
                content = turn
            elif isinstance(turn, dict):
                role = turn.get("role", turn.get("speaker", "user"))
                text = turn.get("content", turn.get("text", ""))
                content = f"[{role}]: {text}"
            else:
                continue
            if not content.strip():
                continue
            client.create(
                content=content,
                tags=[f"lme:item{item_idx}", f"session:{si}", f"turn:{ti}"],
                agent_id=agent_id,
            )
    return agent_id


def evaluate_item(
    client: PlicoClient, item: dict, item_idx: int,
    reader_url: str, reader_model: str,
    judge_url: str, judge_model: str,
) -> list[dict]:
    """Evaluate one LongMemEval item."""
    agent_id = ingest_sessions(client, item, item_idx)
    time.sleep(0.5)

    questions = item.get("questions", item.get("qa_pairs", []))
    if isinstance(questions, dict):
        questions = [questions]

    results = []
    for qa in questions:
        question = qa.get("question", qa.get("q", ""))
        gold = qa.get("answer", qa.get("a", qa.get("gold_answer", "")))
        category = qa.get("category", qa.get("type", "unknown"))

        if not question:
            continue

        resp = client.search(
            query=question,
            agent_id=agent_id,
            limit=5,
            require_tags=[f"lme:item{item_idx}"],
        )
        snippets = [r.get("snippet", "") for r in resp.get("results", []) if r.get("snippet")]
        context = "\n".join(snippets)

        prompt = READER_PROMPT.format(context=context, question=question)
        answer = call_llm(reader_url, reader_model, prompt)

        jp = JUDGE_PROMPT.format(question=question, gold=gold, answer=answer)
        score_str = call_llm(judge_url, judge_model, jp, max_tokens=8)
        try:
            score = int(re.search(r"[1-5]", score_str).group())
        except (AttributeError, ValueError):
            score = 1

        gold_lower = gold.lower().strip()
        answer_lower = answer.lower().strip()
        exact_match = 1.0 if gold_lower in answer_lower or answer_lower in gold_lower else 0.0

        results.append({
            "item_idx": item_idx,
            "category": category,
            "question": question,
            "gold": gold,
            "predicted": answer,
            "llm_score": score,
            "exact_match": exact_match,
            "has_context": len(context) > 0,
        })

    return results


def main():
    parser = argparse.ArgumentParser(description="LongMemEval QA Benchmark for Plico")
    parser.add_argument("--data", default=os.path.join(os.path.dirname(__file__), "..", "data", "longmemeval_s_cleaned.json"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7878)
    parser.add_argument("--reader-url", default="http://127.0.0.1:18920/v1")
    parser.add_argument("--reader-model", default="qwen2.5-7b-instruct")
    parser.add_argument("--judge-url", default=None)
    parser.add_argument("--judge-model", default=None)
    parser.add_argument("--max-items", type=int, default=500)
    parser.add_argument("--output", default=None)
    args = parser.parse_args()

    judge_url = args.judge_url or args.reader_url
    judge_model = args.judge_model or args.reader_model

    items = load_longmemeval(args.data)
    items = items[:args.max_items]
    print(f"Loaded {len(items)} items from {args.data}")

    client = PlicoClient(host=args.host, port=args.port)
    all_results = []

    for i, item in enumerate(items):
        print(f"[{i+1}/{len(items)}] Item {i}...", end=" ", flush=True)
        try:
            results = evaluate_item(
                client, item, i,
                args.reader_url, args.reader_model,
                judge_url, judge_model,
            )
            all_results.extend(results)
            print(f"{len(results)} QAs")
        except Exception as e:
            print(f"ERROR: {e}")
        finally:
            client.close()

    by_category = defaultdict(list)
    for r in all_results:
        by_category[r["category"]].append(r)
        by_category["overall"].append(r)

    print(f"\n{'='*60}")
    print("LongMemEval Results")
    print(f"{'='*60}")

    summary = {}
    for cat, items_list in sorted(by_category.items()):
        avg_score = sum(r["llm_score"] for r in items_list) / len(items_list) if items_list else 0
        avg_em = sum(r["exact_match"] for r in items_list) / len(items_list) if items_list else 0
        accuracy = sum(1 for r in items_list if r["llm_score"] >= 4) / len(items_list) if items_list else 0
        ctx_rate = sum(1 for r in items_list if r["has_context"]) / len(items_list) if items_list else 0
        summary[cat] = {
            "count": len(items_list),
            "llm_score": avg_score,
            "exact_match": avg_em,
            "accuracy_4plus": accuracy,
            "context_hit_rate": ctx_rate,
        }
        print(f"  {cat:20s}  n={len(items_list):3d}  LLM={avg_score:.2f}  EM={avg_em:.3f}  Acc@4+={accuracy:.3f}  ctx={ctx_rate:.2f}")

    output_path = args.output or os.path.join(os.path.dirname(__file__), "longmemeval_results.json")
    with open(output_path, "w") as f:
        json.dump({"summary": summary, "details": all_results}, f, indent=2, ensure_ascii=False)
    print(f"\nResults saved to {output_path}")


if __name__ == "__main__":
    main()
