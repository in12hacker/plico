"""TCP client for plicod — 4-byte BE length-framed JSON."""

from __future__ import annotations

import json
import socket
import struct
import time
import uuid
from typing import Any

MAX_MSG = 16 * 1024 * 1024  # 16 MiB, matches server


class PlicoClient:
    """Thread-safe-ish TCP client for plicod with automatic reconnection."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 7878,
        timeout: float = 300.0,
        max_retries: int = 2,
    ):
        self.host = host
        self.port = port
        self.timeout = timeout
        self.max_retries = max_retries
        self._sock: socket.socket | None = None

    def connect(self) -> None:
        self._sock = socket.create_connection(
            (self.host, self.port), timeout=self.timeout
        )
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def ensure_connected(self) -> None:
        if self._sock is None:
            self.connect()
            return
        try:
            self._sock.getpeername()
        except (OSError, AttributeError):
            self._sock = None
            self.connect()

    def close(self) -> None:
        if self._sock:
            self._sock.close()
            self._sock = None

    def __enter__(self) -> PlicoClient:
        self.connect()
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    def _send(self, data: bytes) -> None:
        assert self._sock is not None
        header = struct.pack(">I", len(data))
        self._sock.sendall(header + data)

    def _recv(self) -> bytes:
        assert self._sock is not None
        header = self._recvn(4)
        length = struct.unpack(">I", header)[0]
        if length > MAX_MSG:
            raise ValueError(f"response too large: {length}")
        return self._recvn(length)

    def _recvn(self, n: int) -> bytes:
        assert self._sock is not None
        buf = bytearray()
        while len(buf) < n:
            chunk = self._sock.recv(n - len(buf))
            if not chunk:
                raise ConnectionError("connection closed")
            buf.extend(chunk)
        return bytes(buf)

    def request(self, req: dict[str, Any]) -> dict[str, Any]:
        self.ensure_connected()
        payload = json.dumps(req, ensure_ascii=False).encode("utf-8")
        for attempt in range(self.max_retries):
            try:
                self._send(payload)
                resp_bytes = self._recv()
                return json.loads(resp_bytes)
            except (ConnectionError, OSError, TimeoutError) as e:
                self._sock = None
                if attempt == self.max_retries - 1:
                    raise
                time.sleep(0.5 * (attempt + 1))
                self.connect()
        raise ConnectionError("max retries exceeded")

    # ── Convenience methods ────────────────────────────────────────

    def health(self) -> dict[str, Any]:
        return self.request({"method": "health_report"})

    def wait_for_indexing(
        self, timeout: float = 120.0, poll_interval: float = 2.0
    ) -> None:
        """Wait until recently written data is searchable.

        Writes a probe item and polls search until it is retrievable.
        This ensures async embedding generation and HNSW index refresh
        have caught up. Call after bulk ingest and before querying.
        """
        probe = f"__bench_probe_{uuid.uuid4().hex}__"
        resp = self.create(probe, tags=["_bench_probe"])
        probe_cid = resp.get("cid", "")
        if not probe_cid:
            # Fallback: heuristic sleep if probe write failed
            time.sleep(min(10.0, timeout))
            return

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                resp = self.search(probe, limit=5, require_tags=["_bench_probe"])
                for h in resp.get("results", []):
                    if h.get("cid") == probe_cid:
                        return
            except Exception:
                pass
            time.sleep(poll_interval)

        raise TimeoutError(
            f"Indexing not complete after {timeout}s (probe cid={probe_cid})"
        )

    def create(
        self, content: str, tags: list[str], agent_id: str = "bench"
    ) -> dict[str, Any]:
        return self.request(
            {
                "method": "create",
                "content": content,
                "tags": tags,
                "agent_id": agent_id,
            }
        )

    def batch_create(
        self, items: list[dict[str, Any]], agent_id: str = "bench"
    ) -> dict[str, Any]:
        return self.request(
            {"method": "batch_create", "items": items, "agent_id": agent_id}
        )

    def read(self, cid: str, agent_id: str = "bench") -> dict[str, Any]:
        return self.request({"method": "read", "cid": cid, "agent_id": agent_id})

    def search(
        self,
        query: str,
        agent_id: str = "bench",
        limit: int = 10,
        require_tags: list[str] | None = None,
        intent: str | None = None,
    ) -> dict[str, Any]:
        req: dict[str, Any] = {
            "method": "search",
            "query": query,
            "agent_id": agent_id,
            "limit": limit,
        }
        if require_tags:
            req["require_tags"] = require_tags
        if intent:
            req["intent"] = intent
        return self.request(req)

    def remember(self, agent_id: str, content: str) -> dict[str, Any]:
        return self.request(
            {"method": "remember", "agent_id": agent_id, "content": content}
        )

    def recall(
        self, agent_id: str, query: str | None = None, limit: int = 10
    ) -> dict[str, Any]:
        req: dict[str, Any] = {
            "method": "recall",
            "agent_id": agent_id,
            "limit": limit,
        }
        if query:
            req["query"] = query
        return self.request(req)

    def recall_semantic(
        self, agent_id: str, query: str, k: int = 10
    ) -> dict[str, Any]:
        return self.request(
            {
                "method": "recall_semantic",
                "agent_id": agent_id,
                "query": query,
                "k": k,
            }
        )

    def recall_routed(self, agent_id: str, query: str, k: int = 10) -> dict[str, Any]:
        return self.request(
            {
                "method": "recall_routed",
                "agent_id": agent_id,
                "query": query,
                "k": k,
            }
        )

    def add_node(
        self,
        label: str,
        node_type: str = "Entity",
        agent_id: str = "bench",
        properties: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        req: dict[str, Any] = {
            "method": "add_node",
            "label": label,
            "node_type": node_type,
            "agent_id": agent_id,
            "properties": properties or {},
        }
        return self.request(req)

    def add_edge(
        self,
        src_id: str,
        dst_id: str,
        edge_type: str = "RelatedTo",
        agent_id: str = "bench",
        weight: float = 1.0,
    ) -> dict[str, Any]:
        return self.request(
            {
                "method": "add_edge",
                "src_id": src_id,
                "dst_id": dst_id,
                "edge_type": edge_type,
                "agent_id": agent_id,
                "weight": weight,
            }
        )

    def find_paths(
        self,
        src_id: str,
        dst_id: str,
        agent_id: str = "bench",
        max_depth: int = 4,
        weighted: bool = False,
    ) -> dict[str, Any]:
        return self.request(
            {
                "method": "find_paths",
                "src_id": src_id,
                "dst_id": dst_id,
                "agent_id": agent_id,
                "max_depth": max_depth,
                "weighted": weighted,
            }
        )

    def start_session(
        self, agent_id: str, goals: list[str] | None = None
    ) -> dict[str, Any]:
        req: dict[str, Any] = {"method": "start_session", "agent_id": agent_id}
        if goals:
            req["goals"] = goals
        return self.request(req)

    def end_session(self, agent_id: str) -> dict[str, Any]:
        return self.request({"method": "end_session", "agent_id": agent_id})

    def remember_long_term(
        self,
        agent_id: str,
        content: str,
        tags: list[str] | None = None,
        importance: int = 5,
    ) -> dict[str, Any]:
        req: dict[str, Any] = {
            "method": "remember_long_term",
            "agent_id": agent_id,
            "content": content,
            "importance": importance,
        }
        if tags:
            req["tags"] = tags
        return self.request(req)
