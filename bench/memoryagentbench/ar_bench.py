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

# Add parent directory to path to import plico_client
sys.path.append(str(Path(__file__).resolve().parent.parent))
from plico_client import PlicoClient

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
    try:
        from sentence_transformers import SentenceTransformer
        from usearch.index import Index
    except ImportError:
        print("WARNING: sentence_transformers or usearch not installed. Skipping offline equivalent mode.")
        model = None
        Index = None

    ar_file = DATA_DIR / "Accurate_Retrieval-00000-of-00001.parquet"
    if not ar_file.exists():
        print(f"ERROR: {ar_file} not found")
        sys.exit(1)

    if model is not None and Index is not None:
        print(f"Loading embedding model: {EMBED_MODEL} ...")
        model = SentenceTransformer(EMBED_MODEL)
    else:
        print("Skipping offline model load.")

    print("Loading MemoryAgentBench AR data ...")
    df = pd.read_parquet(ar_file)
    df = df.head(MAX_SAMPLES)
    print(f"  {len(df)} documents")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    all_results = []
    total_questions = 0
    total_hits = 0
    total_plico_hits = 0

    for di, row in df.iterrows():
        context = row["context"]
        questions = list(row["questions"])
        answers_list = list(row["answers"])
        source = row["metadata"].get("source", "unknown") if isinstance(row["metadata"], dict) else "unknown"

        # Chunk the document
        chunks = chunk_text(context, chunk_chars=2048)
        if not chunks:
            continue
            
        print(f"  [doc {di+1}/{len(df)}] Processing {source} ({len(chunks)} chunks, {len(questions)} questions)...", flush=True)

        # Ingest to Plico
        client = PlicoClient(port=17878)
        agent_id = f"mab-ar-{di}"
        batch_items = [{"content": c, "tags": [f"mab:doc{di}"]} for c in chunks]
        for attempt in range(3):
            try:
                client.batch_create(batch_items, agent_id=agent_id)
                break
            except (ConnectionError, OSError, TimeoutError):
                if attempt < 2:
                    time.sleep(0.5)
                    client.close()

        # Embed chunks
        if model is not None and Index is not None:
            chunk_embeddings = model.encode(chunks, batch_size=64, show_progress_bar=False)

            # Build index
            idx = Index(ndim=EMBED_DIM, metric="cos", dtype="f16")
            for ci, emb in enumerate(chunk_embeddings):
                idx.add(ci, emb.astype(np.float32))
        else:
            idx = None

        doc_hits = 0
        doc_plico_hits = 0
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
            if idx is not None and model is not None:
                query_emb = model.encode(question)
                k = min(5, len(chunks))
                matches = idx.search(query_emb.astype(np.float32), k)

                retrieved_text = " ".join(chunks[int(key)] for key in matches.keys).lower()
                hit = expected.lower() in retrieved_text
            else:
                hit = False
            
            # Plico API Search
            try:
                client = PlicoClient(port=17878)
                resp = client.search(
                    query=question,
                    agent_id=f"mab-ar-{di}",
                    limit=5,
                    require_tags=[f"mab:doc{di}"]
                )
                plico_retrieved = " ".join(r.get("snippet", "") for r in resp.get("results", [])).lower()
                plico_hit = expected.lower() in plico_retrieved
            except Exception as e:
                print(f"Plico search error: {e}")
                plico_hit = False

            if hit:
                doc_hits += 1
                total_hits += 1
            if plico_hit:
                doc_plico_hits += 1
                total_plico_hits += 1
            doc_total += 1
            total_questions += 1

        doc_acc = round(doc_hits / max(doc_total, 1) * 100, 1)
        doc_plico_acc = round(doc_plico_hits / max(doc_total, 1) * 100, 1)
        all_results.append({
            "doc_idx": int(di),
            "source": source,
            "n_chunks": len(chunks),
            "n_questions": doc_total,
            "hits": doc_hits,
            "accuracy": doc_acc,
            "plico_hits": doc_plico_hits,
            "plico_accuracy": doc_plico_acc
        })

        if (di + 1) % 5 == 0:
            running_acc = total_hits / max(total_questions, 1) * 100
            print(f"  [doc {di+1}/{len(df)}] running hit_rate={running_acc:.1f}%")

    overall_acc = round(total_hits / max(total_questions, 1) * 100, 1)

    report = {
        "benchmark": "MemoryAgentBench Accurate Retrieval",
        "system": f"Plico API + Offline equivalent ({EMBED_MODEL})",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": {
            "n_documents": len(all_results),
            "n_questions": total_questions,
            "offline_hits": total_hits,
            "offline_hit_rate": total_hits / max(total_questions, 1) * 100,
            "plico_hits": total_plico_hits,
            "plico_hit_rate": total_plico_hits / max(total_questions, 1) * 100,
            "chunk_size": "2048 chars",
        },
        "per_document": all_results,
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    print("\n" + "=" * 70)
    print("MemoryAgentBench AR Benchmark Results")
    print("=" * 70)
    print("MemoryAgentBench AR Benchmark Results")
    print("=" * 70)
    print(f"  Documents: {len(all_results)}")
    print(f"  Questions: {total_questions}")
    print(f"  Offline Hits: {total_hits}")
    print(f"  Offline Hit Rate: {total_hits / max(total_questions, 1) * 100:.1f}%")
    print(f"  Plico Hits: {total_plico_hits}")
    print(f"  Plico Hit Rate: {total_plico_hits / max(total_questions, 1) * 100:.1f}%")
    print()
    print("Per document:")
    for r in all_results:
        print(f"  [{r['source']:<25}] Plico: {r['plico_accuracy']:>5.1f}% ({r['plico_hits']}/{r['n_questions']}) | Offline: {r['accuracy']:>5.1f}% ({r['hits']}/{r['n_questions']}) chunks={r['n_chunks']}")
    print(f"\nResults saved to: {RESULTS_FILE}")
    return report


if __name__ == "__main__":
    run_benchmark()
