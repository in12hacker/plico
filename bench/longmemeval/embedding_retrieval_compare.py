#!/usr/bin/env python3
"""Compare embedding models on LongMemEval-S retrieval task.

For each question, embed the haystack sessions and query, then measure
recall@5 and recall@10 of finding the answer session(s).
Tests first 50 questions for speed.
"""

import json, time, sys
import urllib.request
import numpy as np
from pathlib import Path

DATA_FILE = Path(__file__).resolve().parent.parent / "data" / "longmemeval_s_cleaned.json"
N_QUESTIONS = 20

MODELS = {
    "qwen2.5-coder-7b (3584d)": {"url": "http://localhost:18920/v1/embeddings", "trunc": None},
    "jina-v5-small (1024d)": {"url": "http://localhost:18921/v1/embeddings", "trunc": None},
    "jina-v5-small (512d)": {"url": "http://localhost:18921/v1/embeddings", "trunc": 512},
    "jina-v5-small (384d)": {"url": "http://localhost:18921/v1/embeddings", "trunc": 384},
    "jina-v5-small (256d)": {"url": "http://localhost:18921/v1/embeddings", "trunc": 256},
}


def embed_batch(url: str, texts: list[str], batch_size: int = 32) -> list[list[float]]:
    all_embs = []
    for i in range(0, len(texts), batch_size):
        batch = texts[i : i + batch_size]
        payload = json.dumps({"input": batch, "model": "test"}).encode()
        req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=60) as resp:
            data = json.loads(resp.read())
        sorted_data = sorted(data["data"], key=lambda x: x["index"])
        all_embs.extend([d["embedding"] for d in sorted_data])
    return all_embs


def truncate_and_normalize(embs: list[list[float]], dim: int) -> np.ndarray:
    arr = np.array([e[:dim] for e in embs])
    norms = np.linalg.norm(arr, axis=1, keepdims=True)
    norms[norms == 0] = 1
    return arr / norms


def cosine_scores(query: np.ndarray, corpus: np.ndarray) -> np.ndarray:
    q = query / (np.linalg.norm(query) + 1e-10)
    return corpus @ q


def session_to_text(session: list[dict]) -> str:
    parts = []
    for turn in session[:6]:
        parts.append(f"{turn['role']}: {turn['content'][:200]}")
    return "\n".join(parts)


def run_model(name: str, config: dict, questions: list[dict]) -> dict:
    url = config["url"]
    trunc = config["trunc"]

    print(f"\n  {name}")
    print(f"  {'─' * 40}")

    recall_at_5 = []
    recall_at_10 = []
    total_embed_time = 0

    for qi, q in enumerate(questions):
        haystack_texts = [session_to_text(s) for s in q["haystack_sessions"]]
        query_text = q["question"]
        answer_ids = set(q["answer_session_ids"])
        session_ids = q["haystack_session_ids"]

        t0 = time.perf_counter()
        all_texts = haystack_texts + [query_text]
        all_embs = embed_batch(url, all_texts)
        total_embed_time += time.perf_counter() - t0

        if trunc:
            emb_array = truncate_and_normalize(all_embs, trunc)
        else:
            emb_array = np.array(all_embs)
            norms = np.linalg.norm(emb_array, axis=1, keepdims=True)
            norms[norms == 0] = 1
            emb_array = emb_array / norms

        corpus_embs = emb_array[:-1]
        query_emb = emb_array[-1]

        scores = cosine_scores(query_emb, corpus_embs)
        ranked_indices = np.argsort(scores)[::-1]

        top5_ids = {session_ids[i] for i in ranked_indices[:5]}
        top10_ids = {session_ids[i] for i in ranked_indices[:10]}

        r5 = len(answer_ids & top5_ids) / len(answer_ids)
        r10 = len(answer_ids & top10_ids) / len(answer_ids)
        recall_at_5.append(r5)
        recall_at_10.append(r10)

        if (qi + 1) % 5 == 0:
            print(f"    {qi+1}/{len(questions)} done (R@5 so far: {np.mean(recall_at_5)*100:.1f}%)...")

    r5_mean = np.mean(recall_at_5) * 100
    r10_mean = np.mean(recall_at_10) * 100
    avg_time = total_embed_time / len(questions) * 1000

    print(f"    R@5:  {r5_mean:.1f}%")
    print(f"    R@10: {r10_mean:.1f}%")
    print(f"    Avg embed time/question: {avg_time:.0f}ms")

    return {
        "name": name,
        "recall_at_5": r5_mean,
        "recall_at_10": r10_mean,
        "avg_embed_ms": avg_time,
    }


def main():
    data = json.load(open(DATA_FILE))
    questions = data[:N_QUESTIONS]
    print(f"LongMemEval-S Embedding Retrieval Comparison ({len(questions)} questions)")
    print("=" * 60)

    results = []
    for name, config in MODELS.items():
        results.append(run_model(name, config, questions))

    print(f"\n\n{'='*80}")
    print("  LongMemEval-S RETRIEVAL SUMMARY")
    print(f"{'='*80}")
    header = f"{'Model':<30} {'R@5':>7} {'R@10':>7} {'EmbMs':>8}"
    print(header)
    print("-" * len(header))
    for r in results:
        print(f"{r['name']:<30} {r['recall_at_5']:>6.1f}% {r['recall_at_10']:>6.1f}% {r['avg_embed_ms']:>7.0f}ms")


if __name__ == "__main__":
    main()
