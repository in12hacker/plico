#!/usr/bin/env python3
"""
Dimension 2: LongMemEval-S End-to-End QA Benchmark

Flow: Store sessions -> Retrieve -> LLM generates answer -> LLM judges accuracy
Uses local llama-server as both reader and judge.

Comparison: Zep 63.8%, Mem0 49%, Oracle GPT-4o ~82.4%
Note: Local 7B judge quality is limited; results are directional only.
"""

import json
import os
import sys
import time
from pathlib import Path

import numpy as np
import requests

BENCH_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = BENCH_ROOT / "data"
RESULTS_DIR = BENCH_ROOT / "results"

# Add bench root to path for judge import
sys.path.insert(0, str(BENCH_ROOT))

DATASET_FILE = DATA_DIR / "longmemeval_s_cleaned.json"
ORACLE_FILE = DATA_DIR / "longmemeval_oracle.json"
RESULTS_FILE = RESULTS_DIR / "longmemeval_e2e.json"

EMBED_MODEL = "all-MiniLM-L6-v2"
EMBED_DIM = 384

LLM_URL = os.environ.get("LLM_URL", "http://127.0.0.1:8080")
LLM_MODEL = os.environ.get("LLM_MODEL", "qwen2.5-coder-7b")
MAX_QUESTIONS = int(os.environ.get("MAX_QUESTIONS", "500"))

READER_PROMPT = """You are a helpful assistant with access to conversation history.
Based on the following retrieved conversation excerpts, answer the question.
If the answer cannot be determined from the excerpts, say "I don't know."

Retrieved excerpts:
{context}

Question: {question}
Answer (be concise):"""

JUDGE_PROMPT = """You are evaluating whether an AI assistant's answer is correct.
Question: {question}
Expected answer: {expected}
AI answer: {actual}

Is the AI answer correct or essentially equivalent to the expected answer?
Reply with ONLY "correct" or "incorrect"."""


def load_model():
    from sentence_transformers import SentenceTransformer
    return SentenceTransformer(EMBED_MODEL)


def flatten_session(session: list[dict]) -> str:
    parts = []
    for turn in session:
        role = turn.get("role", "unknown")
        content = turn.get("content", "")
        parts.append(f"{role}: {content}")
    return "\n".join(parts)


def llm_complete(prompt: str, max_tokens: int = 256) -> str:
    try:
        resp = requests.post(
            f"{LLM_URL}/v1/chat/completions",
            json={
                "model": LLM_MODEL,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": max_tokens,
                "temperature": 0.0,
            },
            timeout=60,
        )
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"].strip()
    except Exception as e:
        return f"[LLM_ERROR: {e}]"


def run_benchmark(use_oracle: bool = False):
    from usearch.index import Index

    src_file = ORACLE_FILE if use_oracle else DATASET_FILE
    if not src_file.exists():
        print(f"ERROR: dataset not found at {src_file}")
        sys.exit(1)

    # Check LLM availability
    try:
        resp = requests.get(f"{LLM_URL}/v1/models", timeout=5)
        resp.raise_for_status()
        print(f"LLM available at {LLM_URL}")
    except Exception:
        print(f"WARNING: LLM not available at {LLM_URL}")
        print("Set LLM_URL env var or start llama-server")
        sys.exit(1)

    print(f"Loading dataset from {src_file} ...")
    with open(src_file) as f:
        dataset = json.load(f)

    dataset = dataset[:MAX_QUESTIONS]
    print(f"  {len(dataset)} questions (max={MAX_QUESTIONS})")

    print(f"Loading embedding model: {EMBED_MODEL} ...")
    model = load_model()

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    all_results = []
    type_metrics: dict[str, list[dict]] = {}
    correct_count = 0

    for qi, item in enumerate(dataset):
        qid = item["question_id"]
        qtype = item.get("question_type", "unknown")
        question = item["question"]
        expected_answer = item.get("answer", "")
        sessions = item.get("haystack_sessions", [])

        if not sessions:
            continue

        # Build session texts
        session_texts = [flatten_session(s) for s in sessions]

        # Embed and build index
        session_embeddings = model.encode(session_texts, batch_size=64, show_progress_bar=False)
        query_embedding = model.encode(question)

        idx = Index(ndim=EMBED_DIM, metric="cos", dtype="f16")
        for sid, emb in zip(range(len(sessions)), session_embeddings):
            idx.add(sid, emb.astype(np.float32))

        # Retrieve top-5 sessions
        matches = idx.search(query_embedding.astype(np.float32), 5)
        retrieved_texts = [session_texts[int(k)] for k in matches.keys]

        # Truncate context to fit 7B model context
        context = "\n---\n".join(retrieved_texts)
        if len(context) > 6000:
            context = context[:6000] + "\n... [truncated]"

        # Generate answer
        reader_prompt = READER_PROMPT.format(context=context, question=question)
        actual_answer = llm_complete(reader_prompt)

        # Judge — use configurable judge interface
        from judge import Judge
        _judge = Judge.from_env()
        judge_result = _judge.evaluate(question, expected_answer, actual_answer)
        is_correct = judge_result.correct
        judgment = judge_result.raw_response

        if is_correct:
            correct_count += 1

        result = {
            "question_id": qid,
            "question_type": qtype,
            "correct": is_correct,
            "expected": expected_answer,
            "actual": actual_answer,
            "judgment": judgment,
        }
        all_results.append(result)

        if qtype not in type_metrics:
            type_metrics[qtype] = []
        type_metrics[qtype].append(result)

        if (qi + 1) % 20 == 0:
            acc = correct_count / (qi + 1) * 100
            print(f"  [{qi+1}/{len(dataset)}] accuracy={acc:.1f}%")

    # Aggregate
    n = len(all_results)
    overall_acc = round(correct_count / max(n, 1) * 100, 1)

    by_type = {}
    for qtype, results in sorted(type_metrics.items()):
        correct = sum(1 for r in results if r["correct"])
        by_type[qtype] = {
            "count": len(results),
            "correct": correct,
            "accuracy": round(correct / max(len(results), 1) * 100, 1),
        }

    mode = "oracle" if use_oracle else "retrieval"
    report = {
        "benchmark": f"LongMemEval-S E2E QA ({mode})",
        "system": f"Plico retrieval + {LLM_MODEL} reader/judge",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": {
            "n_questions": n,
            "correct": correct_count,
            "accuracy": overall_acc,
            "mode": mode,
            "llm_model": LLM_MODEL,
        },
        "by_question_type": by_type,
        "per_question": all_results,
    }

    out_file = RESULTS_DIR / f"longmemeval_e2e_{mode}.json"
    with open(out_file, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    print("\n" + "=" * 70)
    print(f"LongMemEval-S E2E QA Benchmark ({mode})")
    print("=" * 70)
    print(f"System: Plico + {LLM_MODEL}")
    print(f"Questions: {n}, Correct: {correct_count}, Accuracy: {overall_acc}%")
    print()
    print(f"  {'Type':<30} {'Acc':>6} {'n':>5}")
    print(f"  {'─'*30} {'─'*6} {'─'*5}")
    for qtype, m in by_type.items():
        print(f"  {qtype:<30} {m['accuracy']:>5.1f}% {m['count']:>5}")
    print()
    print(f"  Comparison: Zep=63.8%, Mem0=49%, Oracle-GPT4o=82.4%")
    print(f"  NOTE: {LLM_MODEL} as judge — results are directional only")
    print(f"Results saved to: {out_file}")

    return report


if __name__ == "__main__":
    use_oracle = "--oracle" in sys.argv
    run_benchmark(use_oracle=use_oracle)
