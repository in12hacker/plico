#!/usr/bin/env python3
"""Download MemoryAgentBench dataset from HuggingFace."""

from pathlib import Path

DATA_DIR = Path(__file__).resolve().parent.parent / "data" / "memoryagentbench"


def download():
    from huggingface_hub import snapshot_download

    DATA_DIR.mkdir(parents=True, exist_ok=True)

    print("Downloading MemoryAgentBench dataset ...")
    snapshot_download(
        repo_id="ai-hyz/MemoryAgentBench",
        repo_type="dataset",
        local_dir=str(DATA_DIR),
    )
    print(f"Done. Files in {DATA_DIR}")


if __name__ == "__main__":
    download()
