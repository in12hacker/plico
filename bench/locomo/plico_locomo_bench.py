#!/usr/bin/env python3
"""LoCoMo Benchmark for Plico — Long-term Conversational Memory evaluation.

Evaluates Plico on the LoCoMo benchmark from snap-research/locomo.
Flow: Ingest conversation -> Search -> Answer via reader LLM -> Judge via LLM.

Metrics: BLEU-1, F1, LLM-Score per category (single-hop, multi-hop, temporal, open-domain).

Usage:
    python3 plico_locomo_bench.py --data ../data/locomo10.json
    python3 plico_locomo_bench.py --data ../data/locomo10.json --reader-url http://127.0.0.1:18920/v1
"""

import argparse
import json
import os
import re
import sys
import time
from collections import Counter, defaultdict

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from plico_client import PlicoClient

READER_PROMPT = """You are a helpful assistant answering questions based ONLY on the provided context.
If the context does not contain enough information, say "I don't know."

Context:
{context}

Question: {question}

Answer concisely in 1-3 sentences."""

JUDGE_PROMPT = """You are an impartial judge. Rate the following answer on a scale of 1-5 based on its accuracy and completeness compared to the ground truth.

Question: {question}
Ground Truth Answer: {gold}
Generated Answer: {answer}

Scoring:
5 = Perfect match, fully correct and complete
4 = Mostly correct, minor omission
3 = Partially correct, some key info present
2 = Mostly incorrect, only vaguely related
1 = Completely wrong or irrelevant

Output ONLY the numeric score (1-5), nothing else."""


def load_locomo(path: str) -> list[dict]:
    """Load LoCoMo dataset."""
    with open(path, "r", encoding="utf-8") as f:
        data = json.load(f)
    if isinstance(data, dict):
        data = list(data.values()) if not isinstance(list(data.values())[0], list) else list(data.values())[0]
    return data


def ingest_conversation(client: PlicoClient, conv: dict, conv_idx: int) -> str:
    """Ingest a conversation's turns into plicod."""
    agent_id = f"locomo-{conv_idx}"
    conversation = conv.get("conversation", {})

    if isinstance(conversation, dict):
        sessions = sorted(k for k in conversation if k.startswith("session_") and not k.endswith("date_time"))
        turn_idx = 0
        for sess_key in sessions:
            date_key = f"{sess_key}_date_time"
            date_str = conversation.get(date_key, "")
            turns = conversation.get(sess_key, [])
            if not isinstance(turns, list):
                continue
            for turn in turns:
                speaker = turn.get("speaker", "unknown")
                text = turn.get("text", "")
                if not text:
                    continue
                prefix = f"[{date_str}] " if date_str else ""
                content = f"{prefix}[{speaker}]: {text}"
                for attempt in range(3):
                    try:
                        client.create(
                            content=content,
                            tags=[f"locomo:conv{conv_idx}", f"session:{sess_key}", f"turn:{turn_idx}", f"speaker:{speaker}"],
                            agent_id=agent_id,
                        )
                        break
                    except (ConnectionError, OSError):
                        if attempt < 2:
                            time.sleep(0.3)
                            client.close()
                turn_idx += 1
    elif isinstance(conversation, list):
        for i, turn in enumerate(conversation):
            speaker = turn.get("role", turn.get("speaker", "unknown"))
            text = turn.get("content", turn.get("text", ""))
            if not text:
                continue
            content = f"[{speaker}]: {text}"
            client.create(
                content=content,
                tags=[f"locomo:conv{conv_idx}", f"turn:{i}", f"speaker:{speaker}"],
                agent_id=agent_id,
            )

    return agent_id


def search_for_context(client: PlicoClient, question: str, agent_id: str, conv_idx: int, k: int = 5) -> str:
    """Search plicod for relevant context."""
    resp = client.search(
        query=question,
        agent_id=agent_id,
        limit=k,
        require_tags=[f"locomo:conv{conv_idx}"],
    )
    snippets = []
    for r in resp.get("results", []):
        snippet = r.get("snippet", "")
        if snippet:
            snippets.append(snippet)
    return "\n".join(snippets)


def call_llm(url: str, model: str, prompt: str, max_tokens: int = 1024) -> str:
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
            data = json.loads(resp.read())
            msg = data["choices"][0]["message"]
            content = (msg.get("content") or "").strip()
            if not content:
                content = (msg.get("reasoning_content") or "").strip()
            return content
    except Exception as e:
        return f"ERROR: {e}"


def compute_f1(prediction: str, ground_truth: str) -> float:
    """Compute token-level F1."""
    pred_tokens = str(prediction).lower().split()
    gold_tokens = str(ground_truth).lower().split()
    common = Counter(pred_tokens) & Counter(gold_tokens)
    num_same = sum(common.values())
    if num_same == 0:
        return 0.0
    precision = num_same / len(pred_tokens) if pred_tokens else 0
    recall = num_same / len(gold_tokens) if gold_tokens else 0
    return 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0


def compute_bleu1(prediction: str, ground_truth: str) -> float:
    """Compute BLEU-1 (unigram precision)."""
    pred_tokens = prediction.lower().split()
    gold_tokens = ground_truth.lower().split()
    if not pred_tokens:
        return 0.0
    gold_counter = Counter(gold_tokens)
    clipped = 0
    for token in pred_tokens:
        if gold_counter[token] > 0:
            clipped += 1
            gold_counter[token] -= 1
    return clipped / len(pred_tokens)


def evaluate_conversation(
    client: PlicoClient, conv: dict, conv_idx: int,
    reader_url: str, reader_model: str,
    judge_url: str, judge_model: str,
) -> list[dict]:
    """Evaluate all QA pairs for one conversation."""
    agent_id = ingest_conversation(client, conv, conv_idx)
    time.sleep(1)

    qa_pairs = conv.get("qa", conv.get("qa_pairs", conv.get("questions", [])))
    results = []

    cat_names = {1: "single-hop", 2: "temporal", 3: "multi-hop", 4: "open-domain", 5: "adversarial"}
    for qa in qa_pairs:
        question = qa.get("question", qa.get("q", ""))
        gold_answer = str(qa.get("answer", qa.get("a", "")))
        raw_cat = qa.get("category", qa.get("type", "unknown"))
        category = cat_names.get(raw_cat, str(raw_cat)) if isinstance(raw_cat, int) else str(raw_cat)

        if not question or not gold_answer:
            continue

        context = search_for_context(client, question, agent_id, conv_idx)
        prompt = READER_PROMPT.format(context=context, question=question)
        answer = call_llm(reader_url, reader_model, prompt)

        f1 = compute_f1(answer, gold_answer)
        bleu = compute_bleu1(answer, gold_answer)

        judge_prompt = JUDGE_PROMPT.format(question=question, gold=gold_answer, answer=answer)
        score_str = call_llm(judge_url, judge_model, judge_prompt, max_tokens=256)
        try:
            llm_score = int(re.search(r"[1-5]", score_str).group())
        except (AttributeError, ValueError):
            llm_score = 1

        results.append({
            "conv_idx": conv_idx,
            "question": question,
            "gold_answer": gold_answer,
            "predicted_answer": answer,
            "category": category,
            "f1": f1,
            "bleu1": bleu,
            "llm_score": llm_score,
            "has_context": len(context) > 0,
        })

    return results


def main():
    parser = argparse.ArgumentParser(description="LoCoMo Benchmark for Plico")
    parser.add_argument("--data", default=os.path.join(os.path.dirname(__file__), "..", "data", "locomo10.json"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7878)
    parser.add_argument("--reader-url", default="http://127.0.0.1:18920/v1",
                        help="OpenAI-compatible LLM URL for reader")
    parser.add_argument("--reader-model", default="qwen2.5-7b-instruct")
    parser.add_argument("--judge-url", default=None, help="Judge LLM URL (defaults to reader URL)")
    parser.add_argument("--judge-model", default=None, help="Judge model (defaults to reader model)")
    parser.add_argument("--max-conv", type=int, default=10, help="Max conversations to evaluate")
    parser.add_argument("--output", default=None)
    args = parser.parse_args()

    judge_url = args.judge_url or args.reader_url
    judge_model = args.judge_model or args.reader_model

    conversations = load_locomo(args.data)
    conversations = conversations[:args.max_conv]
    print(f"Loaded {len(conversations)} conversations from {args.data}")

    client = PlicoClient(host=args.host, port=args.port)
    all_results = []

    for i, conv in enumerate(conversations):
        print(f"\n[{i+1}/{len(conversations)}] Evaluating conversation {i}...")
        try:
            results = evaluate_conversation(
                client, conv, i,
                args.reader_url, args.reader_model,
                judge_url, judge_model,
            )
            all_results.extend(results)
            print(f"  {len(results)} QA pairs evaluated")
        except Exception as e:
            print(f"  ERROR: {e}")
            import traceback; traceback.print_exc()
        finally:
            client.close()

    by_category = defaultdict(list)
    for r in all_results:
        by_category[r["category"]].append(r)
        by_category["overall"].append(r)

    print(f"\n{'='*60}")
    print("LoCoMo Results")
    print(f"{'='*60}")

    summary = {}
    for cat, items in sorted(by_category.items()):
        avg_f1 = sum(r["f1"] for r in items) / len(items) if items else 0
        avg_bleu = sum(r["bleu1"] for r in items) / len(items) if items else 0
        avg_llm = sum(r["llm_score"] for r in items) / len(items) if items else 0
        context_rate = sum(1 for r in items if r["has_context"]) / len(items) if items else 0
        summary[cat] = {
            "count": len(items),
            "f1": avg_f1,
            "bleu1": avg_bleu,
            "llm_score": avg_llm,
            "context_hit_rate": context_rate,
        }
        print(f"  {cat:20s}  n={len(items):3d}  F1={avg_f1:.3f}  BLEU={avg_bleu:.3f}  LLM={avg_llm:.2f}  ctx={context_rate:.2f}")

    output_path = args.output or os.path.join(os.path.dirname(__file__), "locomo_results.json")
    output = {"summary": summary, "details": all_results}
    with open(output_path, "w") as f:
        json.dump(output, f, indent=2, ensure_ascii=False)
    print(f"\nResults saved to {output_path}")


if __name__ == "__main__":
    main()
