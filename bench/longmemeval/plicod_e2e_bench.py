#!/usr/bin/env python3
"""LongMemEval-S end-to-end benchmark via plicod Search API.

Tests the full BM25+Vector RRF hybrid search pipeline:
  1. Write all haystack sessions as AIObjects into plicod
  2. Search via plicod API for each question
  3. Measure Recall@5 and Recall@10

Usage:
  # Start plicod with jina-v5 embedding first, then:
  python3 bench/longmemeval/plicod_e2e_bench.py [--port PORT] [--questions N]
"""

import argparse
import json
import time
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from plico_client import PlicoClient

DATA_FILE = Path(__file__).resolve().parent.parent / "data" / "longmemeval_s_cleaned.json"


def session_to_text(session: list[dict]) -> str:
    parts = []
    for turn in session[:8]:
        role = turn.get("role", "?")
        content = turn.get("content", "")[:500]
        parts.append(f"{role}: {content}")
    return "\n".join(parts)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=17878)
    parser.add_argument("--questions", type=int, default=30)
    parser.add_argument("--host", default="127.0.0.1")
    args = parser.parse_args()

    data = json.load(open(DATA_FILE))
    questions = data[: args.questions]

    client = PlicoClient(host=args.host, port=args.port, timeout=120)
    client.connect()

    health = client.health()
    print(f"plicod health: ok={health.get('ok')}, version={health.get('version')}")

    # Phase 1: Ingest all unique haystack sessions
    print(f"\n{'='*60}")
    print(f"  Phase 1: Ingesting haystack sessions ({args.questions} questions)")
    print(f"{'='*60}")

    session_cids: dict[str, str] = {}
    total_sessions = 0
    ingest_start = time.perf_counter()

    for qi, q in enumerate(questions):
        for si, (sid, session) in enumerate(
            zip(q["haystack_session_ids"], q["haystack_sessions"])
        ):
            if sid in session_cids:
                continue

            text = session_to_text(session)
            if not text.strip():
                continue

            resp = client.create(
                content=text,
                tags=[f"session:{sid}", f"question:{q['question_id']}"],
                agent_id="longmemeval",
            )
            if resp.get("ok"):
                session_cids[sid] = resp.get("cid", "")
                total_sessions += 1
            else:
                print(f"  WARN: create failed for {sid}: {resp.get('error')}")

        if (qi + 1) % 5 == 0:
            elapsed = time.perf_counter() - ingest_start
            print(
                f"  {qi+1}/{args.questions} questions ingested "
                f"({total_sessions} sessions, {elapsed:.1f}s)"
            )

    ingest_time = time.perf_counter() - ingest_start
    print(
        f"\n  Ingestion complete: {total_sessions} sessions in {ingest_time:.1f}s "
        f"({total_sessions/ingest_time:.1f} sess/s)"
    )

    # Phase 2: Search for each question
    print(f"\n{'='*60}")
    print(f"  Phase 2: Searching ({args.questions} questions)")
    print(f"{'='*60}")

    recall_at_5 = []
    recall_at_10 = []
    search_latencies = []

    for qi, q in enumerate(questions):
        query_text = q["question"]
        answer_sids = set(q["answer_session_ids"])
        haystack_sids = set(q["haystack_session_ids"])

        t0 = time.perf_counter()
        resp = client.search(query_text, agent_id="longmemeval", limit=20)
        lat = (time.perf_counter() - t0) * 1000
        search_latencies.append(lat)

        results = resp.get("results", [])
        result_cids = [r.get("cid", "") for r in results]

        cid_to_sid = {cid: sid for sid, cid in session_cids.items()}

        top5_sids = set()
        top10_sids = set()
        for i, cid in enumerate(result_cids[:10]):
            sid = cid_to_sid.get(cid, "")
            if i < 5:
                top5_sids.add(sid)
            top10_sids.add(sid)

        r5 = len(answer_sids & top5_sids) / max(len(answer_sids), 1)
        r10 = len(answer_sids & top10_sids) / max(len(answer_sids), 1)
        recall_at_5.append(r5)
        recall_at_10.append(r10)

        if (qi + 1) % 5 == 0:
            import numpy as np

            print(
                f"  {qi+1}/{args.questions} "
                f"R@5={np.mean(recall_at_5)*100:.1f}% "
                f"R@10={np.mean(recall_at_10)*100:.1f}% "
                f"p50_lat={np.median(search_latencies):.1f}ms"
            )

    import numpy as np

    # Summary
    print(f"\n\n{'='*60}")
    print(f"  PLICOD E2E LONGMEMEVAL-S RESULTS")
    print(f"{'='*60}")
    print(f"  Questions:       {args.questions}")
    print(f"  Sessions:        {total_sessions}")
    print(f"  Ingest time:     {ingest_time:.1f}s ({total_sessions/ingest_time:.1f} sess/s)")
    print(f"  Recall@5:        {np.mean(recall_at_5)*100:.1f}%")
    print(f"  Recall@10:       {np.mean(recall_at_10)*100:.1f}%")
    print(f"  Search p50 lat:  {np.median(search_latencies):.1f}ms")
    print(f"  Search p99 lat:  {np.percentile(search_latencies, 99):.1f}ms")
    print(f"  Search mean lat: {np.mean(search_latencies):.1f}ms")

    client.close()


if __name__ == "__main__":
    main()
