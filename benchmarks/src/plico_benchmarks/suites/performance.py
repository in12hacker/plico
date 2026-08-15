"""End-to-end latency suite for the public personal protocol."""

from __future__ import annotations

import hashlib
import json
import os
import time
from collections.abc import Callable
from datetime import datetime, timezone
from typing import Any

from plico_benchmarks.core.client import PROTOCOL
from plico_benchmarks.core.config import get_config
from plico_benchmarks.core.metrics import latency_percentiles
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.core.retrieval_execution import (
    real_embedding_required,
    validate_embedding_query,
    validate_retrieval_execution,
    verified_vector_execution,
)
from plico_benchmarks.suites.base import SuiteBase


class PerformanceSuite(SuiteBase):
    name = "performance"
    description = "Public object, working-memory, session, and readiness E2E latency"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        config = self._effective_performance_config()
        self._performance_run_config = config
        self._warmup(config["warmup"])
        results = [self._bench_readiness(config["readiness"])]

        object_put, objects = self._bench_object_put(config["object_put"])
        results.append(object_put)
        results.append(self._bench_object_get(objects, config["object_get"]))
        results.extend(
            self._bench_object_search(
                objects,
                config["object_search"],
                config["search_warm_queries"],
            )
        )

        memory_create, entries = self._bench_memory_create(config["memory_create"])
        results.append(memory_create)
        results.append(self._bench_memory_get(entries, config["memory_get"]))
        results.append(
            self._bench_memory_projection_lag(
                entries,
                timeout=config["projection_timeout_seconds"],
                poll_interval=config["projection_poll_interval_seconds"],
            )
        )
        results.append(self._bench_memory_recall(entries, config["memory_recall"]))
        results.extend(self._bench_memory_mutations(entries, config["memory_mutations"]))
        results.extend(self._bench_sessions(config["session_round_trips"]))
        return results

    def _effective_performance_config(self) -> dict[str, Any]:
        section = get_config().benchmark.get("suites", {}).get("performance", {})
        counts = section.get("default_counts", {})
        required = (
            "readiness",
            "object_put",
            "object_get",
            "object_search",
            "memory_create",
            "memory_get",
            "memory_recall",
            "memory_mutations",
            "session_round_trips",
        )
        missing = [key for key in required if key not in counts]
        if missing:
            raise ValueError(f"performance config missing counts: {', '.join(missing)}")
        effective: dict[str, Any] = {key: self._positive_int(counts[key], key) for key in required}
        warm_queries = section.get("search_warm_queries")
        if (
            not isinstance(warm_queries, list)
            or not warm_queries
            or not all(isinstance(query, str) and query.strip() for query in warm_queries)
        ):
            raise ValueError("performance config search_warm_queries must be non-empty strings")
        effective["search_warm_queries"] = list(warm_queries)
        effective["projection_timeout_seconds"] = self._positive_number(
            section.get("projection_timeout_seconds"), "projection_timeout_seconds"
        )
        effective["projection_poll_interval_seconds"] = self._positive_number(
            section.get("projection_poll_interval_seconds"),
            "projection_poll_interval_seconds",
        )
        if self.samples is not None:
            raise ValueError(
                "performance does not accept a uniform --samples override; "
                "use the versioned per-operation workload config"
            )
        warmup = section.get("warmup")
        if (
            not isinstance(warmup, dict)
            or not isinstance(warmup.get("enabled"), bool)
            or self._positive_int(warmup.get("readiness_requests"), "warmup.readiness_requests")
            <= 0
        ):
            raise ValueError("performance warmup configuration is invalid")
        effective["warmup"] = dict(warmup)
        effective["config_source"] = "configs/benchmark.yaml"
        effective["samples_override"] = self.samples
        return effective

    @staticmethod
    def _positive_int(value: Any, name: str) -> int:
        if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
            raise ValueError(f"performance config {name} must be a positive integer")
        return value

    @staticmethod
    def _positive_number(value: Any, name: str) -> float:
        if isinstance(value, bool) or not isinstance(value, (int, float)) or value <= 0:
            raise ValueError(f"performance config {name} must be positive")
        return float(value)

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        capability_ledger = []
        for result in raw:
            samples = result.get("query_execution_ledger", result.get("sample_ledger", []))
            expected = int(result.get("entries_observed", result.get("count", 0)))
            if not isinstance(samples, list) or len(samples) != expected:
                raise RuntimeError("performance sample ledger count does not match operation")
            capability_ledger.extend(
                {
                    "operation": result["operation"],
                    "capability": "public_protocol_performance",
                    "domain": _operation_domain(result["operation"]),
                    **sample,
                }
                for sample in samples
            )
        return {
            "overall": {
                result["operation"]: {
                    key: value
                    for key, value in result.items()
                    if key not in {"operation", "sample_ledger", "query_execution_ledger"}
                }
                for result in raw
            },
            "capability_ledger": capability_ledger,
        }

    def evaluated_sample_count(self) -> int:
        return sum(self.evaluated_samples_by_operation().values())

    def evaluated_samples_by_operation(self) -> dict[str, int]:
        return {
            str(result["operation"]): int(result.get("entries_observed", result.get("count", 0)))
            for result in self._raw_results
            if not result.get("is_aggregate", False)
        }

    def report(self, metrics: dict[str, Any]) -> Report:
        config = getattr(self, "_performance_run_config", None)
        if config is None:
            config = self._effective_performance_config()
        client = getattr(self, "client", None)
        return Report(
            {
                "metadata": {
                    "suite": self.name,
                    "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                    "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                },
                "config": {
                    "workload_config": config,
                    "latency_boundary": (
                        f"{getattr(client, 'transport', 'tcp')} end-to-end serial requests"
                    ),
                    "search_latency_policy": (
                        "Warm repeated and query-unique workloads are separate after index warmup; "
                        "this is not a cold-start/cache-cold measurement. No universal "
                        "remote-E2E p50<5ms threshold is applied."
                    ),
                    "projection_observability": (
                        "projection.status is polled per created revision; observed six-state, "
                        "unreconciled, and unavailable remain distinct"
                    ),
                },
                "metrics": metrics,
                "costs": {},
                "raw_results": self._raw_results,
            }
        )

    def input_artifacts(self) -> list[dict[str, Any]]:
        config = getattr(self, "_performance_run_config", self._effective_performance_config())
        workload = {
            "schema": "plico.benchmark.performance-workload/v1",
            "protocol": PROTOCOL,
            "seed": self.seed,
            "config": config,
            "operation_matrix": [
                "runtime.readiness",
                "object.put",
                "object.get",
                "object.search_warm_repeated",
                "object.search_query_cold_unique",
                "memory.create",
                "memory.get",
                "projection.status",
                "memory.recall",
                "memory.update",
                "memory.delete",
                "session.start",
                "session.end",
            ],
        }
        payload = json.dumps(workload, sort_keys=True, separators=(",", ":")).encode()
        return [
            {
                "role": "performance_synthetic_workload",
                "file_name": "embedded:performance-workload.json",
                "bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        ]

    def _bench_readiness(self, count: int) -> dict[str, Any]:
        return self._measure(
            "runtime.readiness", count, lambda _index: self.client.runtime_readiness()
        )

    def _warmup(self, config: dict[str, Any]) -> None:
        if config["enabled"]:
            for _ in range(config["readiness_requests"]):
                self.client.runtime_readiness()

    def _bench_object_put(self, count: int) -> tuple[dict[str, Any], list[tuple[str, str]]]:
        objects = []

        def put(index: int) -> None:
            token = f"perfobject{self.run_id}{self.seed}{index}"
            result = self.client.object_put(
                f"Performance object {self.seed}-{index}: machine learning {token}",
                tags=["performance", "object", f"run:{self.run_id}"],
            )
            cid = result.get("cid")
            if not isinstance(cid, str) or not cid:
                raise RuntimeError("object.put returned no CID")
            objects.append((cid, token))

        return self._measure("object.put", count, put), objects

    def _bench_object_get(self, objects: list[tuple[str, str]], count: int) -> dict[str, Any]:
        return self._measure(
            "object.get",
            count,
            lambda index: self.client.object_get(objects[index % len(objects)][0]),
        )

    def _bench_object_search(
        self, objects: list[tuple[str, str]], count: int, warm_queries: list[str]
    ) -> list[dict[str, Any]]:
        self.wait_for_indexing(timeout=getattr(self, "_preprocess_timeout", 120.0))
        for query in warm_queries:
            self.client.object_search(
                query, limit=10, require_tags=["performance", f"run:{self.run_id}"]
            )
        warm_count = max(1, count // 2)
        cold_count = max(1, count - warm_count)
        if cold_count > self._performance_run_config["object_put"]:
            raise ValueError("cold unique search count exceeds prepared unique objects")
        warm = [(warm_queries[index % len(warm_queries)], None) for index in range(warm_count)]
        cold = [(objects[index][1], objects[index][0]) for index in range(cold_count)]
        return [
            self._measure_queries("object.search_warm_repeated", warm, "warm_repeated"),
            self._measure_queries("object.search_query_cold_unique", cold, "query_cold_unique"),
        ]

    def _measure_queries(
        self, operation: str, queries: list[tuple[str, str | None]], workload: str
    ) -> dict[str, Any]:
        latencies = []
        embedding_query_states: dict[str, int] = {}
        retrieval_path_counts: dict[str, int] = {}
        execution_ledger = []
        verified_vector_queries = 0
        degraded_queries = 0
        for query_index, (query, expected_cid) in enumerate(queries):
            started = time.perf_counter()
            result = self.client.object_search(
                query, limit=10, require_tags=["performance", f"run:{self.run_id}"]
            )
            latency_ms = (time.perf_counter() - started) * 1000
            latencies.append(latency_ms)
            hits = result.get("hits")
            if not isinstance(hits, list) or not hits:
                raise RuntimeError("object.search did not return a verified hit")
            if expected_cid is not None and expected_cid not in {
                str(hit.get("cid")) for hit in hits if isinstance(hit, dict)
            }:
                raise RuntimeError("cold object.search missed its seeded canonical target")
            state, embedding_degradation = validate_embedding_query(result.get("embedding_query"))
            retrieval_execution = validate_retrieval_execution(result.get("retrieval"))
            embedding_query_states[state] = embedding_query_states.get(state, 0) + 1
            for item in retrieval_execution:
                path = item["path"]
                retrieval_path_counts[path] = retrieval_path_counts.get(path, 0) + 1
            vector_verified = verified_vector_execution(state, retrieval_execution)
            verified_vector_queries += int(vector_verified)
            degraded = embedding_degradation is not None or any(
                item["degradation"] is not None for item in retrieval_execution
            )
            degraded_queries += int(degraded)
            execution_ledger.append(
                {
                    "query_index": query_index,
                    "latency_ms": latency_ms,
                    "embedding_query_state": state,
                    "embedding_query_degradation": embedding_degradation,
                    "retrieval_execution": retrieval_execution,
                    "expected_target_required": expected_cid is not None,
                    "expected_target_found": expected_cid is None
                    or expected_cid
                    in {str(hit.get("cid")) for hit in hits if isinstance(hit, dict)},
                    "status": "degraded" if degraded else "ok",
                }
            )
        if real_embedding_required() and (
            verified_vector_queries != len(queries) or degraded_queries != 0
        ):
            raise RuntimeError(
                "real-embedding performance run did not prove vector acceptance without degradation"
            )
        fully_verified_vector = (
            bool(queries) and verified_vector_queries == len(queries) and degraded_queries == 0
        )
        return self._latency_result(
            operation,
            latencies,
            workload=workload,
            query_cardinality=len({query for query, _ in queries}),
            includes_query_embedding_stage=True,
            query_embedding_backend=os.environ.get("EMBEDDING_BACKEND", "unknown"),
            embedding_query_states=embedding_query_states,
            retrieval_path_counts=retrieval_path_counts,
            query_execution_ledger=execution_ledger,
            degraded_query_count=degraded_queries,
            verified_vector_query_count=verified_vector_queries,
            retrieval_claim=(
                "verified_vector_execution_latency"
                if fully_verified_vector
                else "object_search_with_query_embedding_latency"
            ),
            status="degraded" if degraded_queries else "measured",
        )

    def _bench_memory_create(self, count: int) -> tuple[dict[str, Any], list[tuple[str, str]]]:
        entries = []

        def create(index: int) -> None:
            token = f"workingmemoryperf{self.run_id}{self.seed}{index}"
            result = self.client.memory_create(
                f"Working memory performance fact {self.seed}-{index} {token}",
                tags=["performance", "working", f"run:{self.run_id}"],
            )
            entry = result.get("entry")
            entry_id = entry.get("entry_id") if isinstance(entry, dict) else None
            if not isinstance(entry_id, str) or not entry_id:
                raise RuntimeError("memory.create returned no entry_id")
            entries.append((entry_id, token))

        result = self._measure("memory.create_ack", count, create)
        result["acknowledgement_boundary"] = "canonical_working_memory_persisted"
        return result, entries

    def _bench_memory_get(self, entries: list[tuple[str, str]], count: int) -> dict[str, Any]:
        return self._measure(
            "memory.get",
            count,
            lambda index: self.client.memory_get(entries[index % len(entries)][0]),
        )

    def _bench_memory_recall(self, entries: list[tuple[str, str]], count: int) -> dict[str, Any]:
        def recall(index: int) -> None:
            entry_id, token = entries[index % len(entries)]
            result = self.client.memory_recall(token, limit=10)
            hits = result.get("hits")
            if not isinstance(hits, list) or not any(
                isinstance(hit, dict)
                and isinstance(hit.get("entry"), dict)
                and hit["entry"].get("entry_id") == entry_id
                for hit in hits
            ):
                raise RuntimeError("memory.recall did not return its seeded canonical target")

        return self._measure("memory.recall_lexical", count, recall)

    def _bench_memory_projection_lag(
        self, entries: list[tuple[str, str]], *, timeout: float, poll_interval: float
    ) -> dict[str, Any]:
        started = time.perf_counter()
        pending = {entry_id for entry_id, _ in entries}
        observations: dict[str, int] = {}
        final_states: dict[str, str] = {}
        ready = 0
        terminal: dict[str, int] = {
            "failed": 0,
            "stale": 0,
            "absent_by_policy": 0,
            "unavailable": 0,
        }
        status_requests = 0
        ready_lag_ms: list[float] = []
        deadline = started + timeout
        while pending and time.perf_counter() < deadline:
            for entry_id in list(pending):
                status = self.client.projection_status(entry_id)
                status_requests += 1
                state = _projection_observation_state(status)
                observations[state] = observations.get(state, 0) + 1
                if state == "ready":
                    pending.remove(entry_id)
                    ready += 1
                    final_states[entry_id] = "ready"
                    ready_lag_ms.append((time.perf_counter() - started) * 1000)
                elif state in terminal:
                    pending.remove(entry_id)
                    terminal[state] += 1
                    final_states[entry_id] = state
            if pending:
                time.sleep(poll_interval)
        elapsed_ms = (time.perf_counter() - started) * 1000
        percentiles = (
            latency_percentiles(ready_lag_ms)
            if ready_lag_ms
            else {
                "p50": None,
                "p95": None,
                "p99": None,
            }
        )
        failed_or_timed_out = sum(terminal.values()) + len(pending)
        for entry_id in pending:
            final_states[entry_id] = "timeout"
        return {
            "operation": "projection.memory_embedding_catch_up",
            "count": status_requests,
            "entries_observed": len(entries),
            "queued": observations.get("queued", 0),
            "building": observations.get("building", 0),
            "unreconciled": observations.get("unreconciled", 0),
            "observation_unit": "projection.status responses",
            "ready": ready,
            **terminal,
            "timeout": len(pending),
            "phase_elapsed_ms": round(elapsed_ms, 3),
            "ready_lag_p50_ms": percentiles["p50"],
            "ready_lag_p95_ms": percentiles["p95"],
            "ready_lag_p99_ms": percentiles["p99"],
            "poll_interval_seconds": poll_interval,
            "timeout_seconds": timeout,
            "status": "partial" if failed_or_timed_out else "measured",
            "failure_count": failed_or_timed_out,
            "p99_gate_eligible": len(ready_lag_ms) >= 1000,
            "sample_ledger": [
                {
                    "sample_index": index,
                    "status": final_states[entry_id],
                }
                for index, (entry_id, _) in enumerate(entries)
            ],
        }

    def _bench_memory_mutations(
        self, entries: list[tuple[str, str]], count: int
    ) -> list[dict[str, Any]]:
        selected = [entry_id for entry_id, _ in entries[: min(count, len(entries))]]
        updated_ids = []
        update_latencies = []
        for index, entry_id in enumerate(selected):
            started = time.perf_counter()
            result = self.client.memory_update(
                entry_id, f"Corrected working memory fact {self.seed}-{index}"
            )
            update_latencies.append((time.perf_counter() - started) * 1000)
            entry = result.get("entry")
            updated_id = entry.get("entry_id") if isinstance(entry, dict) else None
            if not isinstance(updated_id, str) or not updated_id:
                raise RuntimeError("memory.update returned no replacement entry_id")
            updated_ids.append(updated_id)

        delete_latencies = []
        for entry_id in updated_ids:
            started = time.perf_counter()
            self.client.memory_delete(entry_id)
            delete_latencies.append((time.perf_counter() - started) * 1000)
        return [
            self._latency_result("memory.update", update_latencies),
            self._latency_result("memory.delete", delete_latencies),
        ]

    def _bench_sessions(self, count: int) -> list[dict[str, Any]]:
        session_ids = []
        start = self._measure(
            "session.start",
            count,
            lambda _index: session_ids.append(self.client.session_start()["session_id"]),
        )
        end = self._measure(
            "session.end",
            count,
            lambda index: self.client.session_end(session_ids[index]),
        )
        return [start, end]

    def _measure(self, operation: str, count: int, call: Callable[..., Any]) -> dict[str, Any]:
        latencies = []
        for index in range(count):
            started = time.perf_counter()
            call(index)
            latencies.append((time.perf_counter() - started) * 1000)
        return self._latency_result(operation, latencies)

    def _latency_result(
        self, operation: str, latencies: list[float], **metadata: Any
    ) -> dict[str, Any]:
        percentiles = latency_percentiles(latencies)
        elapsed_seconds = sum(latencies) / 1000
        result = {
            "operation": operation,
            "count": len(latencies),
            "serial_service_rate": round(len(latencies) / elapsed_seconds, 3)
            if elapsed_seconds
            else 0.0,
            "rate_unit": "requests/s",
            "p50_ms": percentiles["p50"],
            "p95_ms": percentiles["p95"],
            "p99_ms": percentiles["p99"],
            "p99_gate_eligible": len(latencies) >= 1000,
            "status": "measured",
            **metadata,
        }
        if "query_execution_ledger" not in metadata:
            result["sample_ledger"] = [
                {"sample_index": index, "latency_ms": latency, "status": "ok"}
                for index, latency in enumerate(latencies)
            ]
        return result


def _projection_observation_state(result: dict[str, Any]) -> str:
    if result.get("kind") != "memory_embedding":
        raise RuntimeError("projection.status returned an unexpected projection kind")
    status = result.get("status")
    if not isinstance(status, dict):
        raise RuntimeError("projection.status returned no typed observation")
    observation = status.get("observation")
    if observation in {"unreconciled", "unavailable"}:
        return str(observation)
    if observation != "observed":
        raise RuntimeError("projection.status returned an unknown observation")
    state = status.get("state")
    if not isinstance(state, dict):
        raise RuntimeError("projection.status observed response returned no state")
    state_name = state.get("state")
    if state_name not in {
        "absent_by_policy",
        "queued",
        "building",
        "ready",
        "failed",
        "stale",
    }:
        raise RuntimeError("projection.status returned an unknown manifest state")
    return str(state_name)


def _operation_domain(operation: str) -> str:
    if operation.startswith("object."):
        return "canonical_object_or_object_projection"
    if operation.startswith("memory.") or operation.startswith("projection."):
        return "canonical_memory_or_memory_projection"
    if operation.startswith("session."):
        return "session_lifecycle"
    return "runtime"
