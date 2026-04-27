"""TCP client for plicod — 4-byte BE length-framed JSON."""

import json
import socket
import struct
from typing import Any

MAX_MSG = 16 * 1024 * 1024  # 16 MiB, matches server


class PlicoClient:
    def __init__(self, host: str = "127.0.0.1", port: int = 7878, timeout: float = 60.0):
        self.host = host
        self.port = port
        self.timeout = timeout
        self._sock: socket.socket | None = None

    def connect(self):
        self._sock = socket.create_connection((self.host, self.port), timeout=self.timeout)
        self._sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def ensure_connected(self):
        """Reconnect if the socket is closed."""
        if self._sock is None:
            self.connect()
            return
        try:
            self._sock.getpeername()
        except (OSError, AttributeError):
            self._sock = None
            self.connect()

    def close(self):
        if self._sock:
            self._sock.close()
            self._sock = None

    def __enter__(self):
        self.connect()
        return self

    def __exit__(self, *_):
        self.close()

    def _send(self, data: bytes):
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
        try:
            self._send(payload)
            resp_bytes = self._recv()
            return json.loads(resp_bytes)
        except (ConnectionError, OSError):
            self._sock = None
            self.connect()
            self._send(payload)
            resp_bytes = self._recv()
            return json.loads(resp_bytes)

    # ── Convenience methods ────────────────────────────────────────

    def health(self) -> dict:
        return self.request({"method": "health_report"})

    def create(self, content: str, tags: list[str], agent_id: str = "bench") -> dict:
        return self.request({
            "method": "create",
            "content": content,
            "tags": tags,
            "agent_id": agent_id,
        })

    def read(self, cid: str, agent_id: str = "bench") -> dict:
        return self.request({"method": "read", "cid": cid, "agent_id": agent_id})

    def search(self, query: str, agent_id: str = "bench", limit: int = 10,
               require_tags: list[str] | None = None) -> dict:
        req: dict[str, Any] = {
            "method": "search",
            "query": query,
            "agent_id": agent_id,
            "limit": limit,
        }
        if require_tags:
            req["require_tags"] = require_tags
        return self.request(req)

    def remember(self, agent_id: str, content: str) -> dict:
        return self.request({"method": "remember", "agent_id": agent_id, "content": content})

    def recall(self, agent_id: str, query: str | None = None, limit: int = 10) -> dict:
        req: dict[str, Any] = {"method": "recall", "agent_id": agent_id, "limit": limit}
        if query:
            req["query"] = query
        return self.request(req)

    def recall_semantic(self, agent_id: str, query: str, k: int = 10) -> dict:
        return self.request({
            "method": "recall_semantic",
            "agent_id": agent_id,
            "query": query,
            "k": k,
        })

    def add_node(self, label: str, node_type: str = "Entity", agent_id: str = "bench",
                 properties: dict | None = None) -> dict:
        """node_type: Entity | Fact | Document | Agent | Memory"""
        req: dict[str, Any] = {
            "method": "add_node",
            "label": label,
            "node_type": node_type,
            "agent_id": agent_id,
            "properties": properties or {},
        }
        return self.request(req)

    def add_edge(self, src_id: str, dst_id: str, edge_type: str = "RelatedTo",
                 agent_id: str = "bench", weight: float = 1.0) -> dict:
        """edge_type: AssociatesWith | Follows | Mentions | Causes | Reminds | PartOf | SimilarTo | RelatedTo"""
        return self.request({
            "method": "add_edge",
            "src_id": src_id,
            "dst_id": dst_id,
            "edge_type": edge_type,
            "agent_id": agent_id,
            "weight": weight,
        })

    def find_paths(self, src_id: str, dst_id: str, agent_id: str = "bench",
                   max_depth: int = 4) -> dict:
        return self.request({
            "method": "find_paths",
            "src_id": src_id,
            "dst_id": dst_id,
            "agent_id": agent_id,
            "max_depth": max_depth,
        })

    def start_session(self, agent_id: str, goals: list[str] | None = None) -> dict:
        req: dict[str, Any] = {"method": "start_session", "agent_id": agent_id}
        if goals:
            req["goals"] = goals
        return self.request(req)

    def end_session(self, agent_id: str) -> dict:
        return self.request({"method": "end_session", "agent_id": agent_id})

    def remember_long_term(self, agent_id: str, content: str,
                           tags: list[str] | None = None, importance: int = 5) -> dict:
        req: dict[str, Any] = {
            "method": "remember_long_term",
            "agent_id": agent_id,
            "content": content,
            "importance": importance,
        }
        if tags:
            req["tags"] = tags
        return self.request(req)
