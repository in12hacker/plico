"""Temporal reasoning suite — LongMemEval temporal-reasoning subset."""

from __future__ import annotations

import time
from typing import Any

from plico_benchmarks.core.metrics import bleu1, compute_statistics, token_level_f1
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.datasets.longmemeval import LongMemEvalDataset
from plico_benchmarks.suites.base import SuiteBase

TEMPORAL_PROMPT = """Answer the temporal question using ONLY the context below. Be extremely concise.

Context:
{context}

Question: {question}

Rules:
- Give the specific date/time if available (e.g., "March 5th", "two weeks ago")
- Output ONLY the time/date answer, nothing else
- Do NOT start with "Based on" or "The text says"
- Maximum 10 words

Answer:"""


class TemporalReasoningSuite(SuiteBase):
    name = "temporal-reasoning"
    description = "Temporal reasoning evaluation on LongMemEval TR subset"

    def setup(self) -> None:
        self.wait_for_plico()
        data = LongMemEvalDataset().load()
        self.questions = [
            q for q in data
            if q.get("question_type") == "temporal-reasoning"
        ]
        if not self.questions:
            # Fallback: try any question with date-related keywords
            self.questions = [
                q for q in data
                if any(kw in q.get("question", "").lower() for kw in ["when", "date", "time", "day", "month", "year"])
            ]

    def run(self) -> list[dict[str, Any]]:
        max_q = min(self.samples or 50, len(self.questions))

        # Phase 1: Ingest all haystack sessions
        for item in self.questions[:max_q]:
            sessions = item.get("haystack_sessions", [])
            for sess in sessions:
                if isinstance(sess, list):
                    text = "\n".join(f"{t.get('role','?')}: {t.get('content','')}" for t in sess)
                else:
                    text = str(sess)
                if text.strip():
                    self.client.create(text, tags=["longmemeval", "temporal"])

        # Phase 2: Wait for indexing
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 3: Query
        results = []
        for item in self.questions[:max_q]:
            question = str(item.get("question", ""))
            answer = str(item.get("answer", "")) if item.get("answer") is not None else ""

            resp = self.client.search(question, limit=10)
            hits = resp.get("results", [])
            context = "\n".join(h.get("snippet", "") for h in hits[:5])

            prompt = TEMPORAL_PROMPT.format(context=context, question=question)
            pred = self.llm.chat([{"role": "user", "content": prompt}], max_tokens=32)
            j = self.judge.evaluate(question, answer, pred)

            results.append({
                "question": question,
                "expected": answer,
                "predicted": pred,
                "f1": token_level_f1(pred, answer),
                "bleu1": bleu1(pred, answer),
                "llm_score": 1.0 if j.correct else 0.0,
                "has_context": bool(context.strip()),
            })
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        if not raw:
            return {"overall": {"count": 0}}
        f1s = [r["f1"] for r in raw]
        overall = {
            "count": len(raw),
            "f1": sum(f1s) / len(f1s),
            "bleu1": sum(r["bleu1"] for r in raw) / len(raw),
            "llm_score": sum(r["llm_score"] for r in raw) / len(raw),
            "context_hit_rate": sum(1 for r in raw if r["has_context"]) / len(raw),
            **compute_statistics(f1s),
        }
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v44",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)
