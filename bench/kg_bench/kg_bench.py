#!/usr/bin/env python3
"""
Dimension 4: Knowledge Graph Multi-hop Benchmark

Tests Plico's KG (redb-backed) ability to support multi-hop reasoning
using entities and relationships extracted from LongMemEval conversations.

Evaluates: entity extraction, relationship storage, path finding accuracy.
"""

import json
import os
import re
import sys
import time
from pathlib import Path

import numpy as np
import requests

BENCH_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = BENCH_ROOT / "data"
RESULTS_DIR = BENCH_ROOT / "results"

DATASET_FILE = DATA_DIR / "longmemeval_s_cleaned.json"
RESULTS_FILE = RESULTS_DIR / "kg_multi_hop.json"

LLM_URL = os.environ.get("LLM_URL", "http://127.0.0.1:8080")
LLM_MODEL = os.environ.get("LLM_MODEL", "qwen2.5-coder-7b")

sys.path.insert(0, str(BENCH_ROOT))
from plico_client import PlicoClient

HOST = os.environ.get("PLICO_HOST", "127.0.0.1")
PORT = int(os.environ.get("PLICO_PORT", "7878"))


EXTRACT_PROMPT = """Extract entities and relationships from this conversation.
Output JSON with format: {{"entities": ["name1", "name2"], "relations": [["subject", "predicate", "object"]]}}
Only include important named entities (people, places, organizations, topics).
Keep predicates short (e.g., "works_at", "likes", "mentioned", "discussed").

Conversation:
{text}

JSON output:"""


def llm_complete(prompt: str, max_tokens: int = 512) -> str:
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
        return f'{{"entities": [], "relations": [], "error": "{e}"}}'


def flatten_session(session: list[dict]) -> str:
    parts = []
    for turn in session:
        role = turn.get("role", "unknown")
        content = turn.get("content", "")
        parts.append(f"{role}: {content}")
    return "\n".join(parts)


def extract_entities_relations(text: str) -> tuple[list[str], list[list[str]]]:
    """Use LLM to extract entities and relations from conversation text."""
    if len(text) > 3000:
        text = text[:3000]

    raw = llm_complete(EXTRACT_PROMPT.format(text=text))

    # Strip markdown code block wrapper
    cleaned = re.sub(r'^```(?:json)?\s*', '', raw.strip(), flags=re.MULTILINE)
    cleaned = re.sub(r'```\s*$', '', cleaned.strip(), flags=re.MULTILINE)

    try:
        match = re.search(r'\{.*\}', cleaned, re.DOTALL)
        if match:
            data = json.loads(match.group())
            entities = data.get("entities", [])
            relations = data.get("relations", [])
            return entities, relations
    except json.JSONDecodeError:
        pass
    return [], []


def run_benchmark(max_questions: int = 50):
    """Run KG multi-hop benchmark on a subset of LongMemEval multi-session questions."""

    if not DATASET_FILE.exists():
        print(f"ERROR: dataset not found at {DATASET_FILE}")
        sys.exit(1)

    print(f"Loading dataset ...")
    with open(DATASET_FILE) as f:
        dataset = json.load(f)

    # Filter to multi-session and temporal-reasoning questions
    relevant = [
        item for item in dataset
        if item.get("question_type") in ("multi-session", "temporal-reasoning")
    ]
    relevant = relevant[:max_questions]
    print(f"  {len(relevant)} multi-hop questions selected")

    # Check services
    llm_available = False
    try:
        resp = requests.get(f"{LLM_URL}/v1/models", timeout=5)
        llm_available = resp.status_code == 200
    except Exception:
        pass

    try:
        client = PlicoClient(HOST, PORT, timeout=30)
        client.connect()
        health = client.health()
        print(f"  plicod: ok={health.get('ok')}")
    except Exception as e:
        print(f"ERROR: cannot connect to plicod: {e}")
        sys.exit(1)

    if not llm_available:
        print("WARNING: LLM not available — using heuristic entity extraction")

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    agent = "kg-bench"
    all_results = []
    entities_stored = 0
    edges_stored = 0
    paths_found = 0
    paths_attempted = 0

    for qi, item in enumerate(relevant):
        qid = item["question_id"]
        qtype = item.get("question_type", "unknown")
        question = item["question"]
        gold_session_ids = item.get("answer_session_ids", [])
        sessions = item.get("haystack_sessions", [])

        if not sessions:
            continue

        # Extract entities from evidence sessions
        haystack_ids = item.get("haystack_session_ids", [])
        id_to_idx = {sid: i for i, sid in enumerate(haystack_ids)}
        evidence_sessions = [sessions[id_to_idx[sid]] for sid in gold_session_ids if sid in id_to_idx]

        all_entities = set()
        all_relations = []

        for sess in evidence_sessions[:3]:
            text = flatten_session(sess)
            if llm_available:
                entities, relations = extract_entities_relations(text)
            else:
                # Heuristic: extract capitalized words as entities
                words = set(re.findall(r'\b[A-Z][a-z]+(?:\s[A-Z][a-z]+)*\b', text))
                entities = list(words)[:10]
                relations = []

            all_entities.update(entities)
            all_relations.extend(relations)

        # Store entities in KG
        node_map = {}
        for ent in list(all_entities)[:20]:
            resp = client.add_node(ent, node_type="Entity", agent_id=agent)
            nid = resp.get("node_id")
            if nid:
                node_map[ent] = nid
                entities_stored += 1

        # Map relation predicates to KGEdgeType variants
        EDGE_MAP = {
            "works_at": "AssociatesWith", "likes": "AssociatesWith",
            "mentioned": "Mentions", "discussed": "Mentions",
            "needs": "AssociatesWith", "asks": "AssociatesWith",
            "wears": "AssociatesWith", "plans": "AssociatesWith",
            "related": "RelatedTo", "causes": "Causes",
            "follows": "Follows", "part_of": "PartOf",
        }

        for rel in all_relations[:30]:
            if len(rel) >= 3:
                subj, pred, obj = rel[0], rel[1], rel[2]
                if subj in node_map and obj in node_map:
                    edge_type = EDGE_MAP.get(pred.lower().replace(" ", "_"), "RelatedTo")
                    resp = client.add_edge(node_map[subj], node_map[obj], edge_type=edge_type, agent_id=agent)
                    if resp.get("ok"):
                        edges_stored += 1

        # Test path finding between entity pairs
        entities_list = list(node_map.keys())
        path_results = []
        for i in range(min(3, len(entities_list) - 1)):
            for j in range(i + 1, min(i + 3, len(entities_list))):
                src_id = node_map[entities_list[i]]
                tgt_id = node_map[entities_list[j]]
                paths_attempted += 1
                resp = client.find_paths(src_id, tgt_id, agent_id=agent, max_depth=3)
                found = bool(resp.get("ok") and resp.get("paths"))
                if found:
                    paths_found += 1
                path_results.append({
                    "source": entities_list[i],
                    "target": entities_list[j],
                    "found": found,
                })

        result = {
            "question_id": qid,
            "question_type": qtype,
            "entities_extracted": len(all_entities),
            "relations_extracted": len(all_relations),
            "entities_stored": len(node_map),
            "path_tests": path_results,
        }
        all_results.append(result)

        if (qi + 1) % 10 == 0:
            print(f"  [{qi+1}/{len(relevant)}] entities={entities_stored} edges={edges_stored} paths={paths_found}/{paths_attempted}")

    client.close()

    report = {
        "benchmark": "KG Multi-hop (LongMemEval subset)",
        "system": "Plico KG (redb)",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": {
            "n_questions": len(all_results),
            "total_entities_stored": entities_stored,
            "total_edges_stored": edges_stored,
            "paths_found": paths_found,
            "paths_attempted": paths_attempted,
            "path_hit_rate": round(paths_found / max(paths_attempted, 1) * 100, 1),
            "llm_extraction": llm_available,
        },
        "per_question": all_results,
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    print("\n" + "=" * 70)
    print("KG Multi-hop Benchmark Results")
    print("=" * 70)
    o = report["overall"]
    print(f"  Questions: {o['n_questions']}")
    print(f"  Entities stored: {o['total_entities_stored']}")
    print(f"  Edges stored: {o['total_edges_stored']}")
    print(f"  Path queries: {o['paths_found']}/{o['paths_attempted']} ({o['path_hit_rate']}% hit rate)")
    print(f"  LLM extraction: {o['llm_extraction']}")
    print(f"\nResults saved to: {RESULTS_FILE}")
    return report


if __name__ == "__main__":
    max_q = int(sys.argv[1]) if len(sys.argv) > 1 else 50
    run_benchmark(max_questions=max_q)
