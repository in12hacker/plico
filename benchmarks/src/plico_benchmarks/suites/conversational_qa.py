"""Conversational QA suite — LoCoMo + LongMemEval."""

from __future__ import annotations

import hashlib
import os
import re
import time
import uuid
from collections import Counter
from dataclasses import asdict, is_dataclass
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any

from plico_benchmarks.core.config import get_config
from plico_benchmarks.core.llm_evidence import (
    summarize_llm_costs,
    summarize_llm_identity,
)
from plico_benchmarks.core.llm_journal import (
    JOURNAL_DIR_ENV,
    mark_attempt_journal_complete,
    read_attempt_journal,
)
from plico_benchmarks.core.metrics import accuracy_pct, bleu1, compute_statistics, token_level_f1
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.core.retrieval_execution import (
    provider_identity_scope,
    real_embedding_required,
    validate_embedding_query,
    validate_retrieval_execution,
    verified_vector_execution,
)
from plico_benchmarks.core.sampling import (
    configured_limit,
    configured_profile,
    selection_artifact,
    stable_stratified_sample,
)
from plico_benchmarks.datasets.locomo import LoCoMoDataset
from plico_benchmarks.datasets.longmemeval import LongMemEvalDataset
from plico_benchmarks.suites.base import SuiteBase

READER_PROMPT = """Answer the question using ONLY the context below.

Context:
{context}

Question: {question}

Rules:
- Extract relevant information from the context to answer
- If the context has enough information to make a reasonable inference, give the answer
- If truly no relevant information exists, answer exactly "No information available"
- Be concise — maximum 15 words
- Do NOT start with "Based on" or "The text says"""


def _reader_answer(raw: str) -> str:
    answer = raw.strip()
    if not answer:
        raise RuntimeError("reader returned an empty answer")
    return answer


def _adversarial_abstention_correct(answer: str) -> bool:
    normalized = " ".join(re.sub(r"[^a-z0-9]+", " ", answer.lower()).split())
    return normalized in {"no information available", "not mentioned"}


def _search_execution_evidence(response: dict[str, Any]) -> dict[str, Any]:
    embedding_state, embedding_degradation = validate_embedding_query(
        response.get("embedding_query")
    )
    retrieval_execution = validate_retrieval_execution(response.get("retrieval"))
    vector_verified = verified_vector_execution(embedding_state, retrieval_execution)
    degraded = embedding_degradation is not None or any(
        item["degradation"] is not None for item in retrieval_execution
    )
    if real_embedding_required() and not vector_verified:
        raise RuntimeError(
            "real-embedding conversational QA query did not prove succeeded vector execution"
        )
    return {
        "embedding_query_state": embedding_state,
        "embedding_query_degradation": embedding_degradation,
        "retrieval_execution": retrieval_execution,
        "verified_vector_execution": vector_verified,
        "retrieval_degraded": degraded,
    }


class ConversationalQASuite(SuiteBase):
    name = "conversational-qa"
    description = "LoCoMo + LongMemEval conversational memory QA"

    def setup(self) -> None:
        self.wait_for_plico()
        readiness = self.client.runtime_readiness()
        configured_backend = str(readiness.get("configured_embedding_backend", ""))
        active_provider = str(readiness.get("active_embedding_provider", ""))
        provider_state = str(readiness.get("embedding_provider", ""))
        identity_scope = provider_identity_scope(
            configured_backend,
            active_provider,
            provider_state,
        )
        requirement = (
            "real_non_stub_vector_per_query"
            if real_embedding_required()
            else "typed_execution_observed"
        )
        if requirement == "real_non_stub_vector_per_query" and (
            configured_backend.lower() in {"", "stub", "none", "disabled", "unknown"}
            or identity_scope == "unavailable"
        ):
            raise RuntimeError("real-embedding QA runtime readiness is not verified")
        self._retrieval_runtime = {
            "requirement": requirement,
            "configured_embedding_backend": configured_backend,
            "active_embedding_provider": active_provider,
            "embedding_provider_state": provider_state,
            "provider_identity_scope": identity_scope,
        }
        self._locomo_dataset = LoCoMoDataset()
        self._longmemeval_dataset = LongMemEvalDataset()
        self.locomo = self._locomo_dataset.load()
        self.longmemeval = self._longmemeval_dataset.load()

    def run(self) -> list[dict[str, Any]]:
        section = self._config()
        self._profile = configured_profile(section)
        self._qa_attempt_evidence: list[dict[str, Any]] = []
        self._qa_attempt_keys: set[int] = set()
        self._qa_request_refs: dict[str, list[dict[str, Any]]] = {}
        self._qa_budget_before = self._budget_snapshots()
        locomo_budget, longmemeval_budget = self._sample_limits(section)

        locomo_source = (
            self.locomo if isinstance(self.locomo, list) else self.locomo.get("data", [])
        )
        locomo_candidates = [
            (conv_idx, question_idx, conv, question)
            for conv_idx, conv in enumerate(locomo_source)
            for question_idx, question in enumerate(conv.get("qa", []))
        ]
        self._locomo_sample = stable_stratified_sample(
            locomo_candidates,
            limit=locomo_budget,
            seed=self.seed,
            namespace="conversational-qa:locomo",
            sample_id=lambda item: f"locomo:conv-{item[0]}:qa-{item[1]}",
            stratum=lambda item: self._map_locomo_category(item[3].get("category", 0)),
        )

        longmemeval_data = self.longmemeval if isinstance(self.longmemeval, list) else []
        self._longmemeval_sample = stable_stratified_sample(
            longmemeval_data,
            limit=longmemeval_budget,
            seed=self.seed,
            namespace="conversational-qa:longmemeval",
            sample_id=lambda item: f"longmemeval:{item.get('question_id', '')}",
            stratum=lambda item: str(item.get("question_type", "unknown")),
        )
        selected_ids = [
            *(f"locomo:conv-{item[0]}:qa-{item[1]}" for item in self._locomo_sample),
            *(f"longmemeval:{item.get('question_id', '')}" for item in self._longmemeval_sample),
        ]
        self._selected_sample_ids = selected_ids
        self._selection_artifact = selection_artifact(
            role="conversational_qa_sample_selection",
            seed=self.seed,
            profile=self._profile,
            sample_ids=selected_ids,
        )
        self._qa_config = section

        # Phase 1: ingest exactly the sampled evaluation domains.
        self._ingest_locomo()
        self._ingest_longmemeval()

        # Phase 2: Wait for async indexing (embedding + HNSW)
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 3: Query
        results = []
        results.extend(self._query_locomo())
        results.extend(self._query_longmemeval())
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        from collections import defaultdict

        selected_ids = list(getattr(self, "_selected_sample_ids", []))
        scored_ids = [str(item.get("sample_id", "")) for item in raw]
        if (
            not selected_ids
            or any(not sample_id for sample_id in scored_ids)
            or len(set(scored_ids)) != len(scored_ids)
            or set(scored_ids) != set(selected_ids)
        ):
            raise RuntimeError("QA selected and scored sample identities do not match exactly")

        by_cat: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for r in raw:
            cat = r.get("category", "unknown")
            by_cat[cat].append(r)

        # Accuracy threshold: LLM score >= 4 means "correct" (on 1-5 scale)
        ACCURACY_THRESHOLD = 4

        per_category = {}
        for cat, items in by_cat.items():
            answerable_items = [item for item in items if item["answerability"] == "answerable"]
            adversarial_items = [
                item for item in items if item["answerability"] == "adversarial_unanswerable"
            ]
            f1s = [r["f1"] for r in items if r.get("f1") is not None]
            bleus = [r["bleu1"] for r in items if r.get("bleu1") is not None]
            llms = [r["llm_score"] for r in answerable_items]
            correct = sum(1 for s in llms if s >= ACCURACY_THRESHOLD)
            abstentions = [bool(r["abstention_correct"]) for r in adversarial_items]
            evidence_recalls = [
                r["evidence_recall@10"] for r in items if r.get("evidence_recall@10") is not None
            ]
            per_category[cat] = {
                "count": len(items),
                "answerable_count": len(answerable_items),
                "adversarial_unanswerable_count": len(adversarial_items),
                "f1": sum(f1s) / len(f1s) if f1s else 0.0,
                "bleu1": sum(bleus) / len(bleus) if bleus else 0.0,
                "llm_score": sum(llms) / len(llms) if llms else 0.0,
                "accuracy_pct": round(correct / len(llms) * 100, 1) if llms else 0.0,
                "adversarial_abstention_accuracy_pct": (
                    round(sum(abstentions) / len(abstentions) * 100, 1) if abstentions else None
                ),
                "evidence_recall@10": (
                    sum(evidence_recalls) / len(evidence_recalls) if evidence_recalls else None
                ),
            }

        answerable_raw = [item for item in raw if item["answerability"] == "answerable"]
        adversarial_raw = [
            item for item in raw if item["answerability"] == "adversarial_unanswerable"
        ]
        all_f1 = [r["f1"] for r in raw if r.get("f1") is not None]
        all_bleus = [r["bleu1"] for r in raw if r.get("bleu1") is not None]
        all_llms = [r["llm_score"] for r in answerable_raw]
        all_abstentions = [bool(r["abstention_correct"]) for r in adversarial_raw]
        all_evidence_recalls = [
            r["evidence_recall@10"] for r in raw if r.get("evidence_recall@10") is not None
        ]
        overall = {
            "count": len(raw),
            "answerable_count": len(answerable_raw),
            "adversarial_unanswerable_count": len(adversarial_raw),
            "f1": sum(all_f1) / len(all_f1) if all_f1 else 0.0,
            "bleu1": sum(all_bleus) / len(all_bleus) if all_bleus else 0.0,
            "llm_score": sum(all_llms) / len(all_llms) if all_llms else 0.0,
            "accuracy_pct": accuracy_pct(all_llms),
            "adversarial_abstention_accuracy_pct": (
                round(sum(all_abstentions) / len(all_abstentions) * 100, 1)
                if all_abstentions
                else None
            ),
            "evidence_recall@10": (
                sum(all_evidence_recalls) / len(all_evidence_recalls)
                if all_evidence_recalls
                else None
            ),
            "f1_statistics": compute_statistics(all_f1, seed=self.seed),
        }

        # RAGAS-style LLM-judge proxy on a deterministic 20-item sample. This
        # is not the official RAGAS package and must not be compared as such.
        proxy_limit = int(self._qa_config["ragas_style_proxy_samples"])
        proxy_candidates = answerable_raw
        proxy_sample = (
            stable_stratified_sample(
                proxy_candidates,
                limit=(min(proxy_limit, len(proxy_candidates)) if proxy_candidates else None),
                seed=self.seed,
                namespace="conversational-qa:ragas-style-proxy",
                sample_id=lambda item: str(item["sample_id"]),
                stratum=lambda item: str(item["category"]),
            )
            if proxy_candidates
            else []
        )
        proxy_scores = {
            "faithfulness": [],
            "answer_relevancy": [],
            "context_precision": [],
            "context_recall": [],
        }
        proxy_evaluated = 0
        for item in proxy_sample:
            ctx = item.get("context", "")
            if not ctx:
                continue
            scores = self.judge.evaluate_ragas_style_proxy(
                question=item["question"],
                answer=item["predicted"],
                context=ctx,
                ground_truth=item["expected"],
                sample_id=item["sample_id"],
                request_id=str(uuid.uuid4()),
            )
            self._record_request_evidence(
                item["sample_id"],
                getattr(scores, "role_request_ids", ()),
                getattr(scores, "attempt_evidence", ()),
                boundary="ragas_style_proxy",
            )
            proxy_evaluated += 1
            for k, v in scores.items():
                proxy_scores[k].append(v)
        proxy_metrics = {
            key: round(sum(values) / len(values), 3) if values else 0.0
            for key, values in proxy_scores.items()
        }

        budget_after = self._budget_snapshots()
        costs = self._cost_summary(budget_after)
        journal = self._complete_attempt_journal(costs)
        identity = self._llm_identity_summary()
        evidence_by_sample = {
            sample_id: self._qa_request_refs.get(sample_id, []) for sample_id in scored_ids
        }
        return {
            "overall": overall,
            "per_category": per_category,
            "ragas_style_proxy": proxy_metrics,
            "capability_ledger": [
                {
                    "sample_id": item["sample_id"],
                    "dataset": item["dataset"],
                    "stratum": item["category"],
                    "capability": "conversational_memory_qa",
                    "domain": "plico_object_projection_plus_reader",
                    "status": item.get("status", "ok"),
                    "answerability": item["answerability"],
                    "abstention_correct": item["abstention_correct"],
                    "f1": item["f1"],
                    "bleu1": item["bleu1"],
                    "llm_score": item["llm_score"],
                    "evidence_recall@10": item["evidence_recall@10"],
                    "evidence_recall_counts": {
                        "expected_count": item["evidence_expected_count"],
                        "retrieved_expected_count": item["evidence_retrieved_count"],
                    },
                    "token_overlap": item["token_overlap"],
                    "expected_sha256": item["expected_sha256"],
                    "predicted_sha256": item["predicted_sha256"],
                    "embedding_query_state": item["embedding_query_state"],
                    "embedding_query_degradation": item["embedding_query_degradation"],
                    "retrieval_execution": item["retrieval_execution"],
                    "verified_vector_execution": item["verified_vector_execution"],
                    "retrieval_degraded": item["retrieval_degraded"],
                    "llm_request_evidence": evidence_by_sample[item["sample_id"]],
                }
                for item in raw
            ],
            "sample_accounting": {
                "selected_ids": selected_ids,
                "scored_ids": scored_ids,
                "failed_ids": [],
                "excluded_ids": [],
            },
            "llm_evidence": {
                "schema": "plico.benchmark.deepseek-attempt-ledger/v1",
                "journal": journal,
                "identity": identity,
                "costs": costs,
            },
            "retrieval_runtime": getattr(
                self,
                "_retrieval_runtime",
                {"status": "unavailable_test_double"},
            ),
            "metric_metadata": {
                "answerability": {
                    "answerable": "token_f1_bleu1_and_1_to_5_judge",
                    "adversarial_unanswerable": (
                        "deterministic_no_information_available_or_not_mentioned_abstention"
                    ),
                    "aggregate_rule": "exclude_unanswerable_from_f1_bleu1_and_judge_accuracy",
                },
                "ragas_style_proxy": {
                    "implementation": "custom_single_llm_judge_prompts",
                    "official_ragas": False,
                    "scale": "0.0-1.0",
                    "samples_requested": min(proxy_limit, len(proxy_candidates)),
                    "samples_evaluated": proxy_evaluated,
                    "seed": self.seed,
                    "judge": self.judge.describe(),
                },
            },
        }

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {
                "samples": self.samples,
                "run_id": self.run_id,
                "sampling_profile": self._profile,
                "sampling_strategy": "deterministic_sha256_stratified_v1",
                "evaluation_scope": "single-owner run plus per-sample evidence tags",
            },
            "metrics": metrics,
            "costs": metrics["llm_evidence"]["costs"],
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def input_artifacts(self) -> list[dict[str, Any]]:
        artifacts = [
            self._locomo_dataset.artifact_manifest(),
            self._longmemeval_dataset.artifact_manifest(),
        ]
        selection = getattr(self, "_selection_artifact", None)
        if selection is not None:
            artifacts.append(selection)
        return artifacts

    def external_evidence(self) -> list[dict[str, Any]]:
        summary = getattr(self, "_qa_journal_summary", None)
        if not isinstance(summary, dict) or summary.get("status") != "verified_complete":
            return []
        return [
            {
                "role": "deepseek_paid_attempt_journal",
                "run_id": self.run_id,
                "inventory_sha256": summary["inventory_sha256"],
                "attempt_count": summary["attempt_count"],
                "finalized_attempt_count": summary["finalized_attempt_count"],
                "total_usd_accounted": summary["total_usd_accounted"],
            }
        ]

    def _config(self) -> dict[str, Any]:
        section = get_config().benchmark.get("suites", {}).get("conversational_qa")
        if not isinstance(section, dict):
            raise ValueError("conversational QA configuration is missing")
        if section.get("top_k") != 10:
            raise ValueError("conversational QA evidence top_k must be 10")
        reader_tokens = section.get("reader_max_tokens")
        if (
            isinstance(reader_tokens, bool)
            or not isinstance(reader_tokens, int)
            or reader_tokens <= 0
        ):
            raise ValueError("conversational QA reader token budget must be positive")
        return section

    def _sample_limits(self, section: dict[str, Any]) -> tuple[int | None, int | None]:
        if self.samples is not None:
            if isinstance(self.samples, bool) or self.samples <= 0:
                raise ValueError("samples override must be positive")
            locomo = self.samples // 2
            longmemeval = self.samples - locomo
            return locomo, longmemeval
        return (
            configured_limit(section, profile=self._profile, dataset="locomo", override=None),
            configured_limit(section, profile=self._profile, dataset="longmemeval", override=None),
        )

    # ── LoCoMo ─────────────────────────────────────────────────────

    def _ingest_locomo(self) -> None:
        sample = getattr(self, "_locomo_sample", [])
        if not sample:
            return
        self._locomo_evidence_cids: dict[tuple[int, str], str] = {}
        sampled_conversations = {conv_idx: conv for conv_idx, _, conv, _ in sample}
        for conv_idx, conv in sampled_conversations.items():
            conversation_dict = conv.get("conversation", {})
            if isinstance(conversation_dict, dict):
                # Build session date map: "session_1" -> "8:56 pm on 20 July, 2023"
                session_dates: dict[str, str] = {}
                for key, value in conversation_dict.items():
                    if key.endswith("_date_time") and isinstance(value, str):
                        session_dates[key.replace("_date_time", "")] = value
                # Ingest each session's turns with date context
                for key, value in conversation_dict.items():
                    if (
                        key.startswith("session_")
                        and not key.endswith("_date_time")
                        and isinstance(value, list)
                    ):
                        session_date = session_dates.get(key, "")
                        date_prefix = f"[Date: {session_date}] " if session_date else ""
                        for turn in value:
                            if isinstance(turn, dict):
                                speaker = turn.get("speaker", "User")
                                content = f"{date_prefix}{speaker}: {turn.get('text', '')}"
                                response = self.client.object_put(
                                    content,
                                    tags=[f"run:{self.run_id}", "locomo", f"conv-{conv_idx}"],
                                )
                                cid = self._require_created_cid(response, "LoCoMo turn")
                                dialogue_id = turn.get("dia_id")
                                if dialogue_id:
                                    self._locomo_evidence_cids[(conv_idx, str(dialogue_id))] = cid
            elif isinstance(conversation_dict, list):
                for turn in conversation_dict:
                    if isinstance(turn, dict):
                        content = f"{turn.get('speaker', 'User')}: {turn.get('text', '')}"
                        response = self.client.object_put(
                            content,
                            tags=[f"run:{self.run_id}", "locomo", f"conv-{conv_idx}"],
                        )
                        cid = self._require_created_cid(response, "LoCoMo turn")
                        dialogue_id = turn.get("dia_id")
                        if dialogue_id:
                            self._locomo_evidence_cids[(conv_idx, str(dialogue_id))] = cid

    def _query_locomo(self) -> list[dict[str, Any]]:
        sample = getattr(self, "_locomo_sample", [])
        if not sample:
            return []
        if not hasattr(self, "_qa_config"):
            self._qa_config = self._config()
        results = []
        evidence_cids = getattr(self, "_locomo_evidence_cids", {})
        for conv_idx, question_idx, _, q in sample:
            sample_id = f"locomo:conv-{conv_idx}:qa-{question_idx}"
            question = str(q.get("question", ""))
            category = self._map_locomo_category(q.get("category", 0))
            raw_answer = q.get("answer")
            adversarial_unanswerable = category == "adversarial" and raw_answer is None
            if raw_answer is None and not adversarial_unanswerable:
                raise RuntimeError(f"{sample_id} has no answer outside the adversarial stratum")
            answer = "" if adversarial_unanswerable else str(raw_answer)

            resp = self.client.object_search(
                question,
                limit=self._qa_config["top_k"],
                require_tags=[f"run:{self.run_id}", "locomo", f"conv-{conv_idx}"],
            )
            hits = resp.get("hits", [])
            execution_evidence = _search_execution_evidence(resp)
            context = "\n".join(h.get("snippet", "") for h in hits[:10])
            retrieved_cids = [str(hit.get("cid", "")) for hit in hits[:10]]
            expected_cids = self._resolve_evidence_cids(
                evidence_cids,
                conv_idx,
                q.get("evidence", []),
                f"LoCoMo conv-{conv_idx}:qa-{question_idx}",
            )
            evidence_recall = self._evidence_recall(expected_cids, retrieved_cids)

            prompt = READER_PROMPT.format(context=context, question=question)
            t_online = time.perf_counter()
            max_tok = self._qa_config["reader_max_tokens"]
            reader_request_id = str(uuid.uuid4())
            watermark = self._evidence_watermark(self.llm)
            raw_pred = self.llm.chat(
                [{"role": "user", "content": prompt}],
                max_tokens=max_tok,
                request_id=reader_request_id,
                sample_id=sample_id,
            )
            self._record_request_evidence(
                sample_id,
                (reader_request_id,),
                self._evidence_since(self.llm, watermark, reader_request_id),
                boundary="reader",
                evidence_required=callable(getattr(self.llm, "evidence_since", None)),
            )
            pred = _reader_answer(raw_pred)
            online_lat = (time.perf_counter() - t_online) * 1000

            score = None
            if not adversarial_unanswerable:
                judge_request_id = str(uuid.uuid4())
                judge_result = self.judge.evaluate_scored(
                    question,
                    answer,
                    pred,
                    sample_id=sample_id,
                    request_id=judge_request_id,
                )
                score = _judge_score(judge_result)
                self._record_request_evidence(
                    sample_id,
                    (judge_request_id,),
                    getattr(judge_result, "attempt_evidence", ()),
                    boundary="scored_judge",
                    evidence_required=callable(
                        getattr(getattr(self.judge, "llm", None), "evidence_since", None)
                    ),
                )
            overlap = _token_overlap(pred, answer)
            abstention_correct = (
                _adversarial_abstention_correct(pred) if adversarial_unanswerable else None
            )
            results.append(
                {
                    "dataset": "locomo",
                    "sample_id": sample_id,
                    "category": category,
                    "question": question,
                    "expected": answer,
                    "predicted": pred,
                    "context": context,
                    "answerability": (
                        "adversarial_unanswerable" if adversarial_unanswerable else "answerable"
                    ),
                    "abstention_correct": abstention_correct,
                    "f1": None if adversarial_unanswerable else token_level_f1(pred, answer),
                    "bleu1": None if adversarial_unanswerable else bleu1(pred, answer),
                    "llm_score": score,
                    "evidence_recall@10": evidence_recall,
                    "evidence_expected_count": len(expected_cids),
                    "evidence_retrieved_count": len(expected_cids.intersection(retrieved_cids)),
                    "latency_online_ms": online_lat,
                    "token_overlap": overlap,
                    "expected_sha256": _answer_digest(self.run_id, sample_id, answer),
                    "predicted_sha256": _answer_digest(self.run_id, sample_id, pred),
                    **execution_evidence,
                }
            )
        return results

    # ── LongMemEval ────────────────────────────────────────────────

    def _ingest_longmemeval(self) -> None:
        if not self.longmemeval:
            return
        data = self._longmemeval_sample
        self._longmemeval_evidence_cids: dict[tuple[str, str], str] = {}
        for item_idx, item in enumerate(data):
            question_id = str(item.get("question_id") or f"sample-{item_idx}")
            sessions = item.get("haystack_sessions", [])
            session_ids = item.get("haystack_session_ids", [])
            for session_idx, session in enumerate(sessions):
                if isinstance(session, list):
                    text = "\n".join(
                        f"{t.get('role', '?')}: {t.get('content', '')}"
                        for t in session
                        if isinstance(t, dict)
                    )
                    if text.strip():
                        session_id = str(
                            session_ids[session_idx]
                            if session_idx < len(session_ids)
                            else f"session-{session_idx}"
                        )
                        response = self.client.object_put(
                            text,
                            tags=[
                                f"run:{self.run_id}",
                                "longmemeval",
                                f"question:{question_id}",
                            ],
                        )
                        cid = self._require_created_cid(response, "LongMemEval session")
                        self._longmemeval_evidence_cids[(question_id, session_id)] = cid

    def _query_longmemeval(self) -> list[dict[str, Any]]:
        if not self.longmemeval:
            return []
        if not hasattr(self, "_qa_config"):
            self._qa_config = self._config()
        data = self._longmemeval_sample
        results = []
        evidence_cids = getattr(self, "_longmemeval_evidence_cids", {})
        for item_idx, item in enumerate(data):
            question_id = str(item.get("question_id") or f"sample-{item_idx}")
            sample_id = f"longmemeval:{question_id}"
            question = str(item.get("question", ""))
            if item.get("answer") is None:
                raise RuntimeError(f"{sample_id} has no answer in the answerable dataset")
            answer = str(item["answer"])
            category = item.get("question_type", "unknown")

            resp = self.client.object_search(
                question,
                limit=self._qa_config["top_k"],
                require_tags=[
                    f"run:{self.run_id}",
                    "longmemeval",
                    f"question:{question_id}",
                ],
            )
            hits = resp.get("hits", [])
            execution_evidence = _search_execution_evidence(resp)
            context = "\n".join(h.get("snippet", "") for h in hits[:10])
            retrieved_cids = [str(hit.get("cid", "")) for hit in hits[:10]]
            expected_cids = self._resolve_evidence_cids(
                evidence_cids,
                question_id,
                item.get("answer_session_ids", []),
                f"LongMemEval {question_id}",
            )
            evidence_recall = self._evidence_recall(expected_cids, retrieved_cids)

            prompt = READER_PROMPT.format(context=context, question=question)
            reader_request_id = str(uuid.uuid4())
            watermark = self._evidence_watermark(self.llm)
            raw_pred = self.llm.chat(
                [{"role": "user", "content": prompt}],
                max_tokens=self._qa_config["reader_max_tokens"],
                request_id=reader_request_id,
                sample_id=sample_id,
            )
            self._record_request_evidence(
                sample_id,
                (reader_request_id,),
                self._evidence_since(self.llm, watermark, reader_request_id),
                boundary="reader",
                evidence_required=callable(getattr(self.llm, "evidence_since", None)),
            )
            pred = _reader_answer(raw_pred)
            judge_request_id = str(uuid.uuid4())
            judge_result = self.judge.evaluate_scored(
                question,
                answer,
                pred,
                sample_id=sample_id,
                request_id=judge_request_id,
            )
            score = _judge_score(judge_result)
            self._record_request_evidence(
                sample_id,
                (judge_request_id,),
                getattr(judge_result, "attempt_evidence", ()),
                boundary="scored_judge",
                evidence_required=callable(
                    getattr(getattr(self.judge, "llm", None), "evidence_since", None)
                ),
            )
            overlap = _token_overlap(pred, answer)
            results.append(
                {
                    "dataset": "longmemeval",
                    "sample_id": sample_id,
                    "category": category,
                    "question": question,
                    "expected": answer,
                    "predicted": pred,
                    "context": context,
                    "answerability": "answerable",
                    "abstention_correct": None,
                    "f1": token_level_f1(pred, answer),
                    "bleu1": bleu1(pred, answer),
                    "llm_score": score,
                    "evidence_recall@10": evidence_recall,
                    "evidence_expected_count": len(expected_cids),
                    "evidence_retrieved_count": len(expected_cids.intersection(retrieved_cids)),
                    "token_overlap": overlap,
                    "expected_sha256": _answer_digest(self.run_id, sample_id, answer),
                    "predicted_sha256": _answer_digest(self.run_id, sample_id, pred),
                    **execution_evidence,
                }
            )
        return results

    @staticmethod
    def _require_created_cid(response: dict[str, Any], source: str) -> str:
        cid = response.get("cid")
        if not cid:
            raise RuntimeError(f"{source} ingest did not return a CID: {response!r}")
        return str(cid)

    @staticmethod
    def _evidence_recall(expected_cids: set[str], retrieved_cids: list[str]) -> float | None:
        if not expected_cids:
            return None
        return len(expected_cids.intersection(retrieved_cids)) / len(expected_cids)

    @staticmethod
    def _resolve_evidence_cids(
        evidence_map: dict[tuple[Any, str], str],
        domain_id: Any,
        evidence_ids: list[Any],
        sample_id: str,
    ) -> set[str]:
        normalized_ids = [str(evidence_id) for evidence_id in evidence_ids]
        missing = [
            evidence_id
            for evidence_id in normalized_ids
            if (domain_id, evidence_id) not in evidence_map
        ]
        if missing:
            raise RuntimeError(f"{sample_id} has unmapped evidence IDs: {missing!r}")
        return {evidence_map[(domain_id, evidence_id)] for evidence_id in normalized_ids}

    def _map_locomo_category(self, cat: int) -> str:
        # LoCoMo dataset category IDs:
        # 1=single_hop(282), 2=temporal(321), 3=multi_hop(96),
        # 4=open_domain(841), 5=adversarial(446)
        mapping = {
            1: "single_hop",
            2: "temporal",
            3: "multi_hop",
            4: "open_domain",
            5: "adversarial",
        }
        return mapping.get(cat, "unknown")

    @staticmethod
    def _evidence_watermark(provider: Any) -> int:
        attempts = getattr(provider, "attempts", None)
        if not callable(attempts):
            return 0
        current = attempts()
        return current[-1].attempt_sequence if current else 0

    @staticmethod
    def _evidence_since(provider: Any, watermark: int, request_id: str) -> tuple[Any, ...]:
        evidence_since = getattr(provider, "evidence_since", None)
        if not callable(evidence_since):
            return ()
        return tuple(evidence_since(watermark, role_request_id=request_id))

    def _record_request_evidence(
        self,
        sample_id: str,
        request_ids: tuple[str, ...],
        evidence: tuple[Any, ...],
        *,
        boundary: str,
        evidence_required: bool = True,
    ) -> None:
        if not hasattr(self, "_qa_attempt_evidence"):
            self._qa_attempt_evidence = []
            self._qa_attempt_keys = set()
            self._qa_request_refs = {}
        if evidence_required and not evidence:
            raise RuntimeError(f"{boundary} produced no paid attempt evidence")
        if not evidence:
            self._qa_request_refs.setdefault(sample_id, []).append(
                {
                    "boundary": boundary,
                    "evidence_status": "unavailable_test_double",
                    "request_ids": list(request_ids),
                    "attempt_sequences": [],
                    "usd_accounted": "0",
                }
            )
            return
        allowed_requests = set(request_ids)
        serialized = []
        total = Decimal(0)
        for attempt in evidence:
            record = attempt.to_dict() if hasattr(attempt, "to_dict") else asdict(attempt)
            if record.get("sample_id") != sample_id or record.get("role_request_id") not in (
                allowed_requests
            ):
                raise RuntimeError(f"{boundary} attempt evidence identity mismatch")
            sequence = record.get("attempt_sequence")
            role = record.get("role")
            if (
                isinstance(sequence, bool)
                or not isinstance(sequence, int)
                or sequence <= 0
                or not isinstance(role, str)
                or not role
                or sequence in self._qa_attempt_keys
            ):
                raise RuntimeError(f"{boundary} attempt evidence sequence is invalid")
            self._qa_attempt_keys.add(sequence)
            _verify_attempt_cost(record)
            total += Decimal(record["usd_accounted"])
            serialized.append(record)
        if serialized[-1].get("status") != "ok":
            raise RuntimeError(f"{boundary} has no successful terminal attempt")
        self._qa_attempt_evidence.extend(serialized)
        self._qa_request_refs.setdefault(sample_id, []).append(
            {
                "boundary": boundary,
                "evidence_status": "verified",
                "request_ids": list(request_ids),
                "attempt_sequences": [record["attempt_sequence"] for record in serialized],
                "attempts": [
                    {
                        "attempt_sequence": record["attempt_sequence"],
                        "role": record["role"],
                        "role_request_id": record["role_request_id"],
                        "status": record["status"],
                        "usd_accounted": record["usd_accounted"],
                    }
                    for record in serialized
                ],
                "usd_accounted": format(total, "f"),
            }
        )

    def _budget_snapshots(self) -> dict[str, Any]:
        providers = {
            "reader": self.llm,
            "judge": getattr(self.judge, "llm", None),
        }
        snapshots = {}
        for role, provider in providers.items():
            snapshot = getattr(provider, "budget_snapshot", None)
            if not callable(snapshot):
                snapshots[role] = {"status": "unavailable_test_double"}
                continue
            value = snapshot()
            snapshots[role] = asdict(value) if is_dataclass(value) else dict(value)
        return snapshots

    def _cost_summary(self, budget_after: dict[str, Any]) -> dict[str, Any]:
        role_totals: dict[str, Decimal] = {}
        for record in self._qa_attempt_evidence:
            role = record["role"]
            role_totals[role] = role_totals.get(role, Decimal(0)) + Decimal(record["usd_accounted"])
        for role, after in budget_after.items():
            if after.get("status") == "unavailable_test_double":
                continue
            before = self._qa_budget_before[role]
            delta = Decimal(after["usd_accounted"]) - Decimal(before["usd_accounted"])
            if delta != role_totals.get(role, Decimal(0)):
                raise RuntimeError("LLM budget and per-attempt cost evidence disagree")
        costs = summarize_llm_costs(self._qa_attempt_evidence)
        expected_roles = {role: format(value, "f") for role, value in sorted(role_totals.items())}
        if costs["by_role_usd"] != expected_roles:
            raise RuntimeError("LLM cost summary does not match the attempt ledger")
        return costs

    def _complete_attempt_journal(self, costs: dict[str, Any]) -> dict[str, Any]:
        if not self._qa_attempt_evidence:
            summary = {
                "status": "unavailable_test_double",
                "attempt_count": 0,
                "finalized_attempt_count": 0,
                "total_usd_accounted": "0",
            }
            self._qa_journal_summary = summary
            return summary
        raw_directory = os.environ.get(JOURNAL_DIR_ENV)
        if raw_directory is None:
            raise RuntimeError("paid QA has no durable attempt journal directory")
        journal_directory = Path(raw_directory)
        if journal_directory.name != f"llm-journal-{self.run_id}":
            raise RuntimeError("paid attempt journal directory has no safe run-scoped name")
        snapshot = read_attempt_journal(journal_directory, self.run_id)
        ordered_memory = sorted(
            self._qa_attempt_evidence, key=lambda record: record["attempt_sequence"]
        )
        if (
            snapshot.run_complete
            or snapshot.incomplete_prepared_attempts != 0
            or snapshot.incomplete_pending_files != 0
            or snapshot.attempt_count != len(self._qa_attempt_evidence)
            or snapshot.finalized_attempt_count != snapshot.attempt_count
            or Decimal(snapshot.total_usd_accounted) != Decimal(costs["total_usd"])
            or any(
                entry.phase != "finalized" or entry.finalized != record
                for entry, record in zip(snapshot.entries, ordered_memory, strict=True)
            )
        ):
            raise RuntimeError("paid attempt journal does not match the QA request ledger")
        snapshot = mark_attempt_journal_complete(journal_directory, self.run_id)
        if not snapshot.run_complete:
            raise RuntimeError("paid attempt journal completion is indeterminate")
        summary = {
            "status": "verified_complete",
            "run_id": snapshot.run_id,
            "inventory_sha256": snapshot.inventory_sha256,
            "attempt_count": snapshot.attempt_count,
            "finalized_attempt_count": snapshot.finalized_attempt_count,
            "incomplete_prepared_attempts": snapshot.incomplete_prepared_attempts,
            "incomplete_pending_files": snapshot.incomplete_pending_files,
            "total_usd_accounted": snapshot.total_usd_accounted,
        }
        self._qa_journal_summary = summary
        return summary

    def _llm_identity_summary(self) -> dict[str, Any]:
        if not self._qa_attempt_evidence:
            return {"status": "unavailable_test_double", "roles": {}}
        return summarize_llm_identity(self._qa_attempt_evidence)


def _answer_digest(run_id: str, sample_id: str, answer: str) -> str:
    digest = hashlib.sha256()
    digest.update(b"plico.benchmark.qa-answer.v1\0")
    digest.update(run_id.encode("ascii"))
    digest.update(b"\0")
    digest.update(sample_id.encode("utf-8"))
    digest.update(b"\0")
    digest.update(answer.encode("utf-8"))
    return digest.hexdigest()


def _judge_score(result: Any) -> int:
    score = getattr(result, "score", None)
    if score is None and isinstance(result, tuple) and result:
        score = result[0]
    if isinstance(score, bool) or not isinstance(score, int) or score not in range(1, 6):
        raise RuntimeError("scored judge returned an invalid typed score")
    return score


def _token_overlap(predicted: str, expected: str) -> dict[str, int]:
    def tokens(value: str) -> list[str]:
        normalized = re.sub(r"[^\w\s]", " ", value.lower().strip())
        return re.sub(r"\s+", " ", normalized).split()

    predicted_tokens = tokens(predicted)
    expected_tokens = tokens(expected)
    return {
        "predicted_token_count": len(predicted_tokens),
        "expected_token_count": len(expected_tokens),
        "common_token_count": sum((Counter(predicted_tokens) & Counter(expected_tokens)).values()),
    }


def _verify_attempt_cost(record: dict[str, Any]) -> None:
    try:
        if record.get("usd_basis") == "actual_usage":
            usage = record.get("usage")
            if not isinstance(usage, dict):
                raise ValueError("actual usage is missing")
            recomputed = (
                Decimal(usage["prompt_cache_hit_tokens"])
                * Decimal(record["pricing_cache_hit_per_million_usd"])
                + Decimal(usage["prompt_cache_miss_tokens"])
                * Decimal(record["pricing_cache_miss_per_million_usd"])
                + Decimal(usage["completion_tokens"])
                * Decimal(record["pricing_output_per_million_usd"])
            ) / Decimal(1_000_000)
        elif record.get("usd_basis") == "reserved_upper_bound":
            recomputed = (
                Decimal(record["reserved_input_tokens_upper_bound"])
                * Decimal(record["reservation_cache_miss_per_million_usd"])
                + Decimal(record["reserved_output_tokens"])
                * Decimal(record["reservation_output_per_million_usd"])
            ) / Decimal(1_000_000)
        else:
            raise ValueError("unsupported cost basis")
        if recomputed != Decimal(record["usd_accounted"]):
            raise ValueError("attempt cost does not recompute")
    except (InvalidOperation, KeyError, TypeError, ValueError) as error:
        raise RuntimeError("LLM attempt cost evidence is invalid") from error
