#!/usr/bin/env python3
"""
Dimension 3: MemoryAgentBench Accurate Retrieval Benchmark

Tests chunk-level retrieval accuracy on MemoryAgentBench AR subset.
Format: long document split into chunks -> question -> check if answer in top-K chunks.
"""

import json
import os
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

BENCH_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = BENCH_ROOT / "data" / "memoryagentbench" / "data"
RESULTS_DIR = BENCH_ROOT / "results"
RESULTS_FILE = RESULTS_DIR / "memoryagentbench_ar.json"

EMBED_MODEL = "all-MiniLM-L6-v2"
EMBED_DIM = 384
CHUNK_SIZE = 512  # tokens approx (chars / 4)
MAX_SAMPLES = int(os.environ.get("MAB_MAX_SAMPLES", "20"))


def chunk_text(text: str, chunk_chars: int = 2048) -> list[str]:
    """Split text into chunks by paragraph boundaries."""
    paragraphs = text.split("\n\n")
    chunks = []
    current = ""
    for para in paragraphs:
        if len(current) + len(para) > chunk_chars and current:
            chunks.append(current.strip())
            current = para
        else:
            current = current + "\n\n" + para if current else para
    if current.strip():
        chunks.append(current.strip())
    return chunks if chunks else [text[:chunk_chars]]


def run_benchmark():
    from sentence_transformers import SentenceTransformer
    from usearch.index import Index

    ar_file = DATA_DIR / "Accurate_Retrieval-00000-of-00001.parquet"
    if not ar_file.exists():
        print(f"ERROR: {ar_file} not found")
        sys.exit(1)

    print(f"Loading embedding model: {EMBED_MODEL} ...")
    model = SentenceTransformer(EMBED_MODEL)

    print("Loading MemoryAgentBench AR data ...")
    df = pd.read_parquet(ar_file)
    df = df.head(MAX_SAMPLES)
    print(f"  {len(df)} documents")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    all_results = []
    total_questions = 0
    total_hits = 0

    for di, row in df.iterrows():
        context = row["context"]
        questions = list(row["questions"])
        answers_list = list(row["answers"])
        source = row["metadata"].get("source", "unknown") if isinstance(row["metadata"], dict) else "unknown"

        # Chunk the document
        chunks = chunk_text(context, chunk_chars=2048)
        if not chunks:
            continue

        # Embed chunks
        chunk_embeddings = model.encode(chunks, batch_size=64, show_progress_bar=False)

        # Build index
        idx = Index(ndim=EMBED_DIM, metric="cos", dtype="f16")
        for ci, emb in enumerate(chunk_embeddings):
            idx.add(ci, emb.astype(np.float32))

        doc_hits = 0
        doc_total = 0

        for qi, (question, answer_variants) in enumerate(zip(questions, answers_list)):
            if not question:
                continue

            # Get first valid answer
            if hasattr(answer_variants, '__iter__') and not isinstance(answer_variants, str):
                expected = str(answer_variants[0]) if len(answer_variants) > 0 else ""
            else:
                expected = str(answer_variants)

            if not expected:
                continue

            # Search
            query_emb = model.encode(question)
            k = min(5, len(chunks))
            matches = idx.search(query_emb.astype(np.float32), k)

            retrieved_text = " ".join(chunks[int(key)] for key in matches.keys).lower()
            hit = expected.lower() in retrieved_text

            if hit:
                doc_hits += 1
                total_hits += 1
            doc_total += 1
            total_questions += 1

        doc_acc = round(doc_hits / max(doc_total, 1) * 100, 1)
        all_results.append({
            "doc_idx": int(di),
            "source": source,
            "n_chunks": len(chunks),
            "n_questions": doc_total,
            "hits": doc_hits,
            "accuracy": doc_acc,
        })

        if (di + 1) % 5 == 0:
            running_acc = total_hits / max(total_questions, 1) * 100
            print(f"  [doc {di+1}/{len(df)}] running hit_rate={running_acc:.1f}%")

    overall_acc = round(total_hits / max(total_questions, 1) * 100, 1)

    report = {
        "benchmark": "MemoryAgentBench Accurate Retrieval",
        "system": f"Plico-equivalent (usearch cos f16 + {EMBED_MODEL})",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": {
            "n_documents": len(all_results),
            "n_questions": total_questions,
            "total_hits": total_hits,
            "hit_rate": overall_acc,
            "chunk_size": "2048 chars",
        },
        "per_document": all_results,
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    print("\n" + "=" * 70)
    print("MemoryAgentBench AR Benchmark Results")
    print("=" * 70)
    print(f"  Documents: {len(all_results)}")
    print(f"  Questions: {total_questions}")
    print(f"  Hits: {total_hits}")
    print(f"  Hit Rate: {overall_acc}%")
    print()
    print("Per document:")
    for r in all_results:
        print(f"  [{r['source']:<25}] {r['accuracy']:>5.1f}% ({r['hits']}/{r['n_questions']}) chunks={r['n_chunks']}")
    print(f"\nResults saved to: {RESULTS_FILE}")
    return report


if __name__ == "__main__":
    run_benchmark()
