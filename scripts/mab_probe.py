import sys
sys.path.append("bench")
import json
import pandas as pd
from plico_client import PlicoClient

def test():
    df = pd.read_parquet("bench/data/memoryagentbench/data/Accurate_Retrieval-00000-of-00001.parquet")
    client = PlicoClient()
    client.connect()
    
    # Test first doc
    row = df.iloc[0]
    context = row["context"]
    print(f"Context length: {len(context)}")
    
    # Ingest
    client.create(content=context, tags=["mab:doc0"], agent_id="test")
    print("Ingested")
    
    # Query
    q = row["questions"][0]
    ans = row["answers"][0]
    print(f"Q: {q}")
    print(f"Expected: {ans}")
    
    resp = client.search(query=q, limit=10, require_tags=["mab:doc0"])
    results = resp.get("results", [])
    print(f"Results found: {len(results)}")
    
    snippets = " ".join(r["snippet"].lower() for r in results)
    hit = any(a.lower() in snippets for a in ans)
    print(f"Hit: {hit}")
    if not hit:
        print(f"Snippets: {snippets[:500]}...")

if __name__ == "__main__":
    test()
