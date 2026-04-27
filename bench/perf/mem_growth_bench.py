#!/usr/bin/env python3
"""
Memory Growth Benchmark — tracks plicod RSS after batches of writes.

Measures actual RSS growth per N objects to identify memory leaks.
"""

import json
import os
import socket
import struct
import subprocess
import sys
import time
from pathlib import Path

PLICOD_HOST = "127.0.0.1"
PLICOD_PORT = int(os.environ.get("PLICOD_PORT", "7880"))


def send_recv(sock, req):
    payload = json.dumps(req).encode("utf-8")
    header = struct.pack(">I", len(payload))
    sock.sendall(header + payload)
    raw_len = b""
    while len(raw_len) < 4:
        chunk = sock.recv(4 - len(raw_len))
        if not chunk:
            raise ConnectionError("closed")
        raw_len += chunk
    length = struct.unpack(">I", raw_len)[0]
    data = b""
    while len(data) < length:
        chunk = sock.recv(min(length - len(data), 65536))
        if not chunk:
            raise ConnectionError("closed")
        data += chunk
    return json.loads(data)


def get_rss_mb(pid):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) / 1024  # KB to MB
    except Exception:
        pass
    return 0.0


def main():
    total_writes = int(sys.argv[1]) if len(sys.argv) > 1 else 500
    batch_size = int(sys.argv[2]) if len(sys.argv) > 2 else 50
    content_size = int(sys.argv[3]) if len(sys.argv) > 3 else 1000

    # Find plicod PID
    result = subprocess.run(["pgrep", "-f", f"plicod start.*{PLICOD_PORT}"],
                          capture_output=True, text=True)
    pids = [p for p in result.stdout.strip().split("\n") if p]
    if not pids:
        # Fallback: find any plicod
        result = subprocess.run(["pgrep", "-f", "plicod start"],
                              capture_output=True, text=True)
        pids = [p for p in result.stdout.strip().split("\n") if p]
    pid = int(pids[0]) if pids else None
    if not pid:
        print("ERROR: Cannot find plicod PID")
        sys.exit(1)

    initial_rss = get_rss_mb(pid)
    print(f"plicod PID={pid}, initial RSS={initial_rss:.1f}MB")
    print(f"Plan: {total_writes} writes, batch={batch_size}, content={content_size} bytes")

    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.settimeout(30)
    sock.connect((PLICOD_HOST, PLICOD_PORT))

    measurements = [{"n": 0, "rss_mb": initial_rss, "delta_mb": 0}]

    for batch_start in range(0, total_writes, batch_size):
        batch_end = min(batch_start + batch_size, total_writes)
        for i in range(batch_start, batch_end):
            content = f"Memory test document {i}. " + ("A" * content_size)
            resp = send_recv(sock, {
                "method": "create",
                "content": content,
                "tags": ["mem-test", f"batch-{batch_start // batch_size}"],
                "agent_id": "mem-bench",
            })
            if not resp.get("ok"):
                print(f"  Write {i} FAILED: {resp.get('error')}")
                break

        rss = get_rss_mb(pid)
        delta = rss - initial_rss
        measurements.append({
            "n": batch_end,
            "rss_mb": round(rss, 1),
            "delta_mb": round(delta, 1),
        })
        bytes_per_obj = (delta * 1024 * 1024) / batch_end if batch_end > 0 and delta > 0 else 0
        print(f"  After {batch_end:>5} writes: RSS={rss:>8.1f}MB  delta={delta:>+8.1f}MB  "
              f"~{bytes_per_obj:.0f} bytes/obj")

    sock.close()

    final_rss = get_rss_mb(pid)
    total_growth = final_rss - initial_rss
    per_object = (total_growth * 1024 * 1024) / total_writes if total_writes > 0 else 0

    print(f"\n{'='*60}")
    print(f"Memory Growth Summary")
    print(f"{'='*60}")
    print(f"  Objects written: {total_writes}")
    print(f"  Content size:    {content_size} bytes/obj")
    print(f"  Initial RSS:     {initial_rss:.1f} MB")
    print(f"  Final RSS:       {final_rss:.1f} MB")
    print(f"  Growth:          {total_growth:.1f} MB")
    print(f"  Per object:      {per_object:.0f} bytes")
    print(f"  Growth rate:     {total_growth / total_writes * 1000:.1f} KB per 1000 objects")

    out = Path(__file__).resolve().parent.parent / "results" / "mem_growth.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        json.dump({"measurements": measurements, "summary": {
            "total_writes": total_writes,
            "content_size": content_size,
            "initial_rss_mb": initial_rss,
            "final_rss_mb": final_rss,
            "growth_mb": total_growth,
            "per_object_bytes": per_object,
        }}, f, indent=2)
    print(f"\nResults saved to {out}")


if __name__ == "__main__":
    main()
