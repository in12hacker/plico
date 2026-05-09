#!/usr/bin/env python3
"""
Dimension 3: MemoryAgentBench Accurate Retrieval Benchmark

Tests chunk-level retrieval accuracy on MemoryAgentBench AR subset.
Format: long document split into chunks -> question -> check if answer is in top-k chunks.
"""

import json
import os
import sys
import time
from pathlib import Path

import numpy as np
from usearch.index import Index

BENCH_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = BENCH_ROOT / "data"
RESULTS_DIR = BENCH_ROOT / "results"

DATASET_FILE = DATA_DIR / "memoryagentbench_ar.json"
RESULTS_FILE = RESULTS_DIR / "memoryagentbench_ar.json"

sys.path.insert(0, str(BENCH_ROOT))
from plico_client import PlicoClient

HOST = os.environ.get("PLICO_HOST", "127.0.0.1")
PORT = int(os.environ.get("PLICO_PORT", "7878"))


def run_benchmark():
    if not DATASET_FILE.exists():
        parquet_file = DATA_DIR / "memoryagentbench" / "data" / "Accurate_Retrieval-00000-of-00001.parquet"
        if not parquet_file.exists():
            print(f"ERROR: dataset not found at {DATASET_FILE} or {parquet_file}")
            return
        print(f"Loading dataset from {parquet_file} ...")
        import pandas as pd
        df = pd.read_parquet(parquet_file)
        # Convert parquet format to docs format expected by the script
        docs = []
        for _, row in df.iterrows():
            docs.append({
                "source": row.get("metadata", {}).get("key", "unknown"),
                "chunks": [row["context"]], # Parquet AR seems to be 1 doc = 1 context
                "questions": [{"question": q, "answers": row["answers"][i]} for i, q in enumerate(row["questions"])]
            })
    else:
        print(f"Loading dataset from {DATASET_FILE} ...")
        with open(DATASET_FILE) as f:
            data = json.load(f)
        docs = data if isinstance(data, list) else data.get("documents", [])
    
    client = PlicoClient(HOST, PORT, timeout=30)
    try:
        client.connect()
        print(f"Connected to plicod at {HOST}:{PORT}")
    except Exception as e:
        print(f"ERROR: cannot connect to plicod: {e}")
        return

    all_results = []
    total_hits = 0
    total_plico_hits = 0
    total_questions = 0

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    # Setup offline index for comparison (all-MiniLM-L6-v2)
    # Note: This is just a simulation if real embeddings aren't available
    
    for di, doc in enumerate(docs):
        chunks = doc.get("chunks", [])
        questions = doc.get("questions", [])
        source = doc.get("source", "unknown")
        
        if not chunks or not questions:
            continue
            
        print(f"  [{di+1}/{len(docs)}] doc={source} chunks={len(chunks)} questions={len(questions)}")
        
        # 1. Ingest doc chunks into Plico
        agent_id = f"mab-ar-{di}"
        for ci, text in enumerate(chunks):
            client.create(
                content=text,
                tags=[f"mab:doc{di}", f"mab:chunk{ci}", f"mab:source:{source}"],
                agent_id=agent_id
            )
        
        doc_hits = 0
        doc_plico_hits = 0
        doc_total = 0
        
        for q_item in questions:
            question = q_item["question"]
            expected_answers = q_item.get("answers", []) # List of correct answers
            
            # Offline Search Simulation (Mock for now to focus on Plico)
            hit = False 
            
            # Plico API Search
            try:
                resp = client.search(
                    query=question,
                    agent_id=agent_id,
                    limit=10, # Increase limit for large docs
                    require_tags=[f"mab:doc{di}"]
                )
                if resp.get("ok") and resp.get("results"):
                    plico_retrieved = " ".join(r.get("snippet", "").lower() for r in resp.get("results"))
                    # Check if any of the expected answers appear in retrieved text
                    plico_hit = any(ans.lower() in plico_retrieved for ans in expected_answers)
                else:
                    plico_hit = False
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
        print(f"    Acc: Plico={doc_plico_acc}%")

    report = {
        "benchmark": "MemoryAgentBench Accurate Retrieval",
        "system": "Plico API",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": {
            "n_documents": len(all_results),
            "n_questions": total_questions,
            "hits": total_hits,
            "accuracy": round(total_hits / max(total_questions, 1) * 100, 1),
            "plico_hits": total_plico_hits,
            "plico_accuracy": round(total_plico_hits / max(total_questions, 1) * 100, 1)
        },
        "details": all_results
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2)

    print("\n" + "=" * 70)
    print("MemoryAgentBench AR Results")
    print("=" * 70)
    r = report["overall"]
    print(f"  Total Questions: {total_questions}")
    print(f"  Plico Hits: {total_plico_hits}/{total_questions} ({r['plico_accuracy']}%)")
    print(f"\nResults saved to: {RESULTS_FILE}")
    
    client.close()
    return report


if __name__ == "__main__":
    run_benchmark()
