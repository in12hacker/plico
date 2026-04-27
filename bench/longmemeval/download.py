#!/usr/bin/env python3
"""Download LongMemEval-S cleaned dataset from HuggingFace."""

import os
from pathlib import Path

DATA_DIR = Path(__file__).resolve().parent.parent / "data"


def download():
    from huggingface_hub import hf_hub_download

    DATA_DIR.mkdir(parents=True, exist_ok=True)

    files = [
        "longmemeval_s_cleaned.json",
        "longmemeval_oracle.json",
    ]

    for fname in files:
        dest = DATA_DIR / fname
        if dest.exists():
            print(f"  {fname} already exists, skipping")
            continue
        print(f"  Downloading {fname} ...")
        hf_hub_download(
            repo_id="xiaowu0162/longmemeval-cleaned",
            filename=fname,
            repo_type="dataset",
            local_dir=str(DATA_DIR),
        )
        print(f"  -> {dest}")

    print("Done.")


if __name__ == "__main__":
    download()
