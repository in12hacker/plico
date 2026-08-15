"""Typed ``plico.personal.v2`` TCP/UDS client for plicod."""

from __future__ import annotations

import json
import os
import socket
import struct
import time
import uuid
from typing import Any

from plico_benchmarks.core.retrieval_execution import (
    real_embedding_required,
    validate_embedding_query,
    validate_retrieval_execution,
    verified_vector_execution,
)

MAX_MSG = 16 * 1024 * 1024
PROTOCOL = "plico.personal.v2"
PUBLIC_OPERATION_CATALOG = (
    "capabilities.describe",
    "runtime.readiness",
    "object.put",
    "object.get",
    "object.search",
    "memory.create",
    "memory.get",
    "memory.recall",
    "projection.status",
    "projection.rebuild",
    "memory.update",
    "memory.delete",
    "session.start",
    "session.end",
)
PUBLIC_OPERATIONS = frozenset(PUBLIC_OPERATION_CATALOG)
READ_ONLY_OPERATIONS = frozenset(
    {
        "capabilities.describe",
        "runtime.readiness",
        "object.get",
        "object.search",
        "memory.get",
        "memory.recall",
        "projection.status",
    }
)
PUBLIC_ERROR_CODES = frozenset(
    {
        "INVALID_ARGUMENT",
        "UNAUTHENTICATED",
        "PERMISSION_DENIED",
        "NOT_FOUND",
        "CONFLICT",
        "LIMIT_EXCEEDED",
        "BUSY",
        "PROVIDER_UNAVAILABLE",
        "DEPENDENCY_UNAVAILABLE",
        "UNSUPPORTED_CAPABILITY",
        "INTERNAL",
    }
)
PUBLIC_INPUT_FIELDS = {
    "capabilities.describe": frozenset(),
    "runtime.readiness": frozenset(),
    "object.put": frozenset({"content", "encoding", "tags"}),
    "object.get": frozenset({"cid"}),
    "object.search": frozenset({"query", "limit", "require_tags", "exclude_tags"}),
    "memory.create": frozenset({"content", "tags"}),
    "memory.get": frozenset({"entry_id"}),
    "memory.recall": frozenset({"query", "limit"}),
    "projection.status": frozenset({"kind", "revision_id"}),
    "projection.rebuild": frozenset({"kind", "selector"}),
    "memory.update": frozenset({"entry_id", "content"}),
    "memory.delete": frozenset({"entry_id"}),
    "session.start": frozenset({"last_seen_seq"}),
    "session.end": frozenset({"session_id"}),
}


class PlicoProtocolError(RuntimeError):
    """The peer returned an invalid envelope or a typed domain failure."""

    def __init__(self, message: str, *, code: str | None = None):
        super().__init__(message)
        self.code = code


class PlicoClient:
    """Fail-closed client for the exact 14-operation personal protocol."""

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 7878,
        timeout: float = 300.0,
        max_retries: int = 2,
        bearer_token: str | None = None,
        uds_path: str | None = None,
    ):
        if max_retries < 1:
            raise ValueError("max_retries must be at least 1")
        self.host = host
        self.port = port
        self.timeout = timeout
        self.max_retries = max_retries
        self._bearer_token = bearer_token or os.environ.get("PLICO_BEARER_TOKEN") or None
        self.uds_path = uds_path
        self._sock: socket.socket | None = None
        self.last_exchange_bytes: dict[str, int] | None = None
        self.last_exchange: dict[str, Any] | None = None

    @property
    def transport(self) -> str:
        return "uds" if self.uds_path is not None else "tcp"

    def connect(self) -> None:
        if self.uds_path is not None:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.settimeout(self.timeout)
            sock.connect(self.uds_path)
            self._sock = sock
        else:
            self._require_bearer()
            self._sock = socket.create_connection((self.host, self.port), timeout=self.timeout)
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

    def request(self, operation: str, input_data: dict[str, Any]) -> dict[str, Any]:
        if operation not in PUBLIC_OPERATIONS:
            raise ValueError(f"operation is not in the public capability catalog: {operation}")
        unknown_fields = set(input_data) - PUBLIC_INPUT_FIELDS[operation]
        if unknown_fields:
            fields = ", ".join(sorted(unknown_fields))
            raise ValueError(f"{operation} input contains fields outside its schema: {fields}")
        request_id = str(uuid.uuid4())
        envelope = {
            "protocol": PROTOCOL,
            "request_id": request_id,
            "operation": operation,
            "input": input_data,
        }
        if self.uds_path is None:
            envelope["auth"] = {"bearer": self._require_bearer()}
        payload = json.dumps(envelope, ensure_ascii=False, separators=(",", ":")).encode()
        if not payload or len(payload) > MAX_MSG:
            raise PlicoProtocolError("request frame is outside the public protocol size limit")

        attempts = self.max_retries if operation in READ_ONLY_OPERATIONS else 1
        self.last_exchange = {
            "request_id": request_id,
            "wire_operation": operation,
            "attempt_count": 0,
            "frame_sent": False,
            "response_observed": False,
            "request": len(payload) + 4,
            "response": 0,
        }
        for attempt in range(attempts):
            try:
                self.last_exchange["attempt_count"] = attempt + 1
                self.ensure_connected()
                self._send(payload)
                self.last_exchange["frame_sent"] = True
                response_payload = self._recv()
                self.last_exchange["response_observed"] = True
                self.last_exchange["response"] = len(response_payload) + 4
                self.last_exchange_bytes = {
                    "request": len(payload) + 4,
                    "response": len(response_payload) + 4,
                }
                response = json.loads(response_payload)
                return self._validated_result(response, request_id, operation)
            except (ConnectionError, OSError, TimeoutError):
                self.close()
                if attempt == attempts - 1:
                    raise
                time.sleep(0.5 * (attempt + 1))
        raise ConnectionError("maximum reconnect attempts exceeded")

    def _require_bearer(self) -> str:
        if not self._bearer_token:
            raise PlicoProtocolError("PLICO_BEARER_TOKEN is required for TCP benchmark requests")
        if len(self._bearer_token.encode()) > 4096:
            raise PlicoProtocolError("PLICO_BEARER_TOKEN exceeds the public protocol limit")
        return self._bearer_token

    def _send(self, data: bytes) -> None:
        assert self._sock is not None
        self._sock.sendall(struct.pack(">I", len(data)) + data)

    def _recv(self) -> bytes:
        header = self._recvn(4)
        length = struct.unpack(">I", header)[0]
        if length == 0 or length > MAX_MSG:
            raise PlicoProtocolError("response frame length is outside the protocol limit")
        return self._recvn(length)

    def _recvn(self, size: int) -> bytes:
        assert self._sock is not None
        payload = bytearray()
        while len(payload) < size:
            chunk = self._sock.recv(size - len(payload))
            if not chunk:
                raise ConnectionError("connection closed before the frame completed")
            payload.extend(chunk)
        return bytes(payload)

    @staticmethod
    def _validated_result(response: Any, request_id: str, operation: str) -> dict[str, Any]:
        if not isinstance(response, dict):
            raise PlicoProtocolError(f"{operation} returned a non-object response")
        if response.get("protocol") != PROTOCOL:
            raise PlicoProtocolError(f"{operation} returned an unexpected protocol")
        if response.get("request_id") != request_id:
            raise PlicoProtocolError(f"{operation} returned a mismatched request_id")
        ok = response.get("ok")
        if ok is True:
            if set(response) != {"protocol", "request_id", "ok", "data"}:
                raise PlicoProtocolError(f"{operation} returned an invalid success envelope")
        elif ok is False:
            if set(response) != {"protocol", "request_id", "ok", "error"}:
                raise PlicoProtocolError(f"{operation} returned an invalid error envelope")
            error = response.get("error")
            code = error.get("code") if isinstance(error, dict) else "INVALID_RESPONSE"
            if code not in PUBLIC_ERROR_CODES:
                code = "INVALID_RESPONSE"
            raise PlicoProtocolError(f"{operation} failed [{code}]", code=code)
        else:
            raise PlicoProtocolError(f"{operation} returned a non-boolean ok field")
        data = response.get("data")
        if not isinstance(data, dict) or data.get("operation") != operation:
            raise PlicoProtocolError(f"{operation} returned mismatched typed data")
        result = data.get("result")
        if not isinstance(result, dict):
            raise PlicoProtocolError(f"{operation} returned a non-object typed result")
        return result

    # Exact public capability conveniences.

    def capabilities_describe(self) -> dict[str, Any]:
        return self.request("capabilities.describe", {})

    def runtime_readiness(self) -> dict[str, Any]:
        return self.request("runtime.readiness", {})

    def cognitive_progress(self) -> dict[str, int]:
        """Read the coherent cognitive-pipeline progress snapshot."""
        readiness = self.runtime_readiness()
        progress = readiness.get("cognitive_progress")
        if not isinstance(progress, dict):
            raise PlicoProtocolError("runtime.readiness returned no cognitive_progress snapshot")
        expected = {"accepted", "completed", "in_flight"}
        if set(progress) != expected:
            raise PlicoProtocolError("runtime.readiness returned malformed cognitive_progress")
        values: dict[str, int] = {}
        for field in sorted(expected):
            value = progress[field]
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise PlicoProtocolError("runtime.readiness returned malformed cognitive_progress")
            values[field] = value
        if values["completed"] > values["accepted"]:
            raise PlicoProtocolError("runtime.readiness returned inconsistent cognitive_progress")
        return values

    def wait_for_cognitive_watermark(
        self,
        accepted_watermark: int,
        *,
        timeout: float = 120.0,
        poll_interval: float = 0.2,
    ) -> dict[str, int]:
        """Wait until every task through one accepted ingest watermark finishes."""
        if (
            isinstance(accepted_watermark, bool)
            or not isinstance(accepted_watermark, int)
            or accepted_watermark < 0
        ):
            raise ValueError("accepted watermark must be a non-negative integer")
        deadline = time.monotonic() + timeout
        last: dict[str, int] | None = None
        while time.monotonic() < deadline:
            last = self.cognitive_progress()
            if last["completed"] >= accepted_watermark:
                return last
            time.sleep(poll_interval)
        completed = None if last is None else last["completed"]
        raise TimeoutError(
            "cognitive indexing did not reach accepted watermark "
            f"{accepted_watermark} after {timeout}s (completed={completed})"
        )

    def object_put(self, content: str, tags: list[str] | None = None) -> dict[str, Any]:
        return self.request(
            "object.put", {"content": content, "encoding": "utf8", "tags": tags or []}
        )

    def object_get(self, cid: str) -> dict[str, Any]:
        return self.request("object.get", {"cid": cid})

    def object_search(
        self,
        query: str,
        limit: int = 10,
        require_tags: list[str] | None = None,
        exclude_tags: list[str] | None = None,
    ) -> dict[str, Any]:
        return self.request(
            "object.search",
            {
                "query": query,
                "limit": limit,
                "require_tags": require_tags or [],
                "exclude_tags": exclude_tags or [],
            },
        )

    def memory_create(self, content: str, tags: list[str] | None = None) -> dict[str, Any]:
        return self.request("memory.create", {"content": content, "tags": tags or []})

    def memory_get(self, entry_id: str) -> dict[str, Any]:
        return self.request("memory.get", {"entry_id": entry_id})

    def memory_recall(self, query: str, limit: int = 10) -> dict[str, Any]:
        return self.request("memory.recall", {"query": query, "limit": limit})

    def projection_status(self, revision_id: str) -> dict[str, Any]:
        return self.request(
            "projection.status",
            {"kind": "memory_embedding", "revision_id": revision_id},
        )

    def projection_rebuild_current(self, revision_id: str) -> dict[str, Any]:
        return self.request(
            "projection.rebuild",
            {
                "kind": "memory_embedding",
                "selector": {"type": "current_revision", "revision_id": revision_id},
            },
        )

    def projection_rebuild_all_eligible(self) -> dict[str, Any]:
        return self.request(
            "projection.rebuild",
            {"kind": "memory_embedding", "selector": {"type": "all_eligible"}},
        )

    def memory_update(self, entry_id: str, content: str) -> dict[str, Any]:
        return self.request("memory.update", {"entry_id": entry_id, "content": content})

    def memory_delete(self, entry_id: str) -> dict[str, Any]:
        return self.request("memory.delete", {"entry_id": entry_id})

    def session_start(self, last_seen_seq: int | None = None) -> dict[str, Any]:
        input_data = {} if last_seen_seq is None else {"last_seen_seq": last_seen_seq}
        return self.request("session.start", input_data)

    def session_end(self, session_id: str) -> dict[str, Any]:
        return self.request("session.end", {"session_id": session_id})

    def wait_for_object_indexing(self, timeout: float = 120.0, poll_interval: float = 2.0) -> None:
        """Wait for one public object probe to become searchable."""
        probe = f"__bench_probe_{uuid.uuid4().hex}__"
        cid = self.object_put(probe, tags=["_bench_probe"])["cid"]
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            response = self.object_search(probe, limit=5, require_tags=["_bench_probe"])
            hits = response.get("hits")
            if not isinstance(hits, list):
                raise PlicoProtocolError("object.search returned no hits list")
            if any(isinstance(hit, dict) and hit.get("cid") == cid for hit in hits):
                if real_embedding_required() and not _response_proves_vector_execution(response):
                    time.sleep(poll_interval)
                    continue
                return
            time.sleep(poll_interval)
        raise TimeoutError(f"object indexing did not complete after {timeout}s")


def _response_proves_vector_execution(response: dict[str, Any]) -> bool:
    state, _ = validate_embedding_query(response.get("embedding_query"))
    retrieval = validate_retrieval_execution(response.get("retrieval"))
    return verified_vector_execution(state, retrieval)
