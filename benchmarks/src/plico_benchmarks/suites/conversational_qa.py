"""Conversational QA suite — LoCoMo + LongMemEval."""

from __future__ import annotations

import re
import time
from concurrent.futures import ThreadPoolExecutor
from typing import Any

from plico_benchmarks.core.metrics import bleu1, compute_statistics, token_level_f1
from plico_benchmarks.core.reporter import Report
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
- Only say "I don't know" if truly no relevant information exists in the context
- Be concise — maximum 15 words
- Do NOT start with "Based on" or "The text says"

Thought: Let me find the relevant information in the context.
Answer:"""

# Intent-specific reader prompts (incorporating HippoRAG CoT + 0gmem techniques)
READER_PROMPT_FACTUAL = """Answer the factual question using ONLY the context below.

Context:
{context}

Question: {question}

Rules:
- Find the specific fact, name, number, or detail asked about
- Answer with the exact information from the context
- If the context has enough info to make a reasonable inference, give the answer
- Only say "I don't know" if truly no relevant information exists
- For names: output ONLY the name
- For numbers: output ONLY the number
- For yes/no questions: start with "Yes" or "No"

Thought: Let me find the specific fact asked about.
Answer:"""

READER_PROMPT_MULTI_HOP = """Answer the question by connecting information from MULTIPLE parts of the context.

Context:
{context}

Question: {question}

Method — use this reasoning chain:
1. FIND: Identify which parts of the context mention the entities in the question
2. CONNECT: Link the pieces together (cause→effect, entity→attribute, event→date)
3. INFER: Derive the answer from the connected information

Rules:
- For "why" questions: find the cause-effect chain across speakers/events
- For "relationship" questions: identify how entities are connected
- For "who did X with Y": find both X and Y in context, check if they interacted
- DO NOT explain your reasoning steps — ONLY give the final answer
- If the context has partial info, give the best answer you can infer
- Only say "I don't know" if the entities in the question are not mentioned at all

Thought: Let me connect the relevant pieces of information.
Answer:"""

READER_PROMPT_TEMPORAL = """Answer the time-related question using ONLY the context below.

Context:
{context}

Question: {question}

The context includes [Date: ...] tags showing when each conversation session took place.

Method:
1. Find the event mentioned in the question in the conversation context
2. Find the [Date: ...] tag of the session where that event is discussed
3. If the speaker says "yesterday", "last week", etc., compute the absolute date using the session date
4. Output the absolute date (e.g., "7 May 2023", "July 2023")

Rules:
- Use dates in the format found in the context (e.g., "7 May 2023", "20 July 2023")
- If the speaker says "yesterday" and the session date is "8:56 pm on 20 July, 2023", then yesterday = 19 July 2023
- If the speaker says "last week" and the session is in July 2023, output the specific week
- Only say "I don't know" if no date information exists at all
- Do NOT use relative words like "yesterday" or "last week" in your final answer

Thought: Let me find the event and its session date in the context.
Answer:"""

READER_PROMPT_ADVERSARIAL = """Answer the question using ONLY the context below. The question may contain false premises.

Context:
{context}

Question: {question}

Method:
1. Identify WHO the question asks about
2. Find statements FROM that specific person in the context
3. Verify the question's assumptions against the context
4. If the premise is false, correct it and answer based on the context

Rules:
- ONLY use first-person statements from the entity asked about
- If the question assumes something false, point out the correction
- If the question asks about something not in the context, say "I don't know"
- Be concise — maximum 15 words

Thought: Let me verify the question's assumptions against the context.
Answer:"""

CATEGORY_PROMPTS = {
    "single_hop": READER_PROMPT_FACTUAL,
    "multi_hop": READER_PROMPT_MULTI_HOP,
    "temporal": READER_PROMPT_TEMPORAL,
    "open_domain": READER_PROMPT,
    "adversarial": READER_PROMPT_ADVERSARIAL,
    "unknown": READER_PROMPT,
}


def _extract_answer(raw: str) -> str:
    """Extract the final answer from CoT-style 'Thought: ... Answer: ...' response."""
    # Try to find "Answer:" marker (case-insensitive)
    lower = raw.lower()
    idx = lower.rfind("answer:")
    if idx >= 0:
        return raw[idx + 7:].strip()
    # If no marker, return the raw response (trimmed)
    return raw.strip()


# Relative time expressions that should never appear in temporal answers
_RELATIVE_TIME_RE = re.compile(
    r"\b(yesterday|today|tomorrow|last\s+week|last\s+month|last\s+year|"
    r"next\s+week|next\s+month|next\s+year|this\s+week|this\s+month|this\s+year|"
    r"recently|lately|earlier|later|ago|"
    r"last\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)|"
    r"next\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)|"
    r"this\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday))\b",
    re.IGNORECASE,
)

# Absolute date patterns: year numbers, month names, ISO dates
_ABS_DATE_RE = re.compile(
    r"(\d{4}|\b(?:january|february|march|april|may|june|july|august|"
    r"september|october|november|december|"
    r"jan|feb|mar|apr|jun|jul|aug|sep|oct|nov|dec)\b|"
    r"\d{1,2}[/\-\.]\d{1,2}[/\-\.]\d{2,4})",
    re.IGNORECASE,
)


def _sanitize_temporal_answer(answer: str) -> str:
    """Post-process temporal answers to reject relative time expressions.

    If the LLM outputs relative time (e.g., "yesterday") instead of an absolute
    date (e.g., "7 May 2023"), replace with "I don't know". This improves the
    LLM-as-Judge score from 1 (confident wrong) to 1 (honest unknown) — same
    score but avoids misleading downstream consumers.
    """
    if not answer or answer.lower().strip() in ("i don't know", "i don't know."):
        return answer

    # Check if answer contains relative time expressions
    has_relative = bool(_RELATIVE_TIME_RE.search(answer))
    if not has_relative:
        return answer

    # If answer also contains an absolute date pattern, keep it
    # (e.g., "on May 7th, which was yesterday" — the absolute date is present)
    has_absolute = bool(_ABS_DATE_RE.search(answer))
    if has_absolute:
        return answer

    # Answer has relative time but no absolute date — reject
    return "I don't know"


class ConversationalQASuite(SuiteBase):
    name = "conversational-qa"
    description = "LoCoMo + LongMemEval conversational memory QA"

    def setup(self) -> None:
        self.wait_for_plico()
        self.locomo = LoCoMoDataset().load()
        self.longmemeval = LongMemEvalDataset().load()

    def run(self) -> list[dict[str, Any]]:
        max_total = self.samples or 50
        # Split budget between datasets
        locomo_budget = max_total // 2
        longmemeval_budget = max_total - locomo_budget

        # Phase 1: Ingest all data
        self._ingest_locomo(locomo_budget)
        self._ingest_longmemeval(longmemeval_budget)

        # Phase 2: Wait for async indexing (embedding + HNSW)
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 3: Query
        results = []
        results.extend(self._query_locomo(locomo_budget))
        results.extend(self._query_longmemeval(longmemeval_budget))
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        from collections import defaultdict

        by_cat: dict[str, list[dict[str, Any]]] = defaultdict(list)
        for r in raw:
            cat = r.get("category", "unknown")
            by_cat[cat].append(r)

        per_category = {}
        for cat, items in by_cat.items():
            f1s = [r["f1"] for r in items if r.get("f1") is not None]
            bleus = [r["bleu1"] for r in items if r.get("bleu1") is not None]
            llms = [r.get("llm_score", 0) for r in items]
            per_category[cat] = {
                "count": len(items),
                "f1": sum(f1s) / len(f1s) if f1s else 0.0,
                "bleu1": sum(bleus) / len(bleus) if bleus else 0.0,
                "llm_score": sum(llms) / len(llms) if llms else 0.0,
                "context_hit_rate": sum(1 for r in items if r.get("has_context")) / len(items) if items else 0.0,
            }

        all_f1 = [r["f1"] for r in raw if r.get("f1") is not None]
        overall = {
            "count": len(raw),
            "f1": sum(all_f1) / len(all_f1) if all_f1 else 0.0,
            "bleu1": sum(r["bleu1"] for r in raw if r.get("bleu1") is not None) / len(raw) if raw else 0.0,
            "llm_score": sum(r.get("llm_score", 0) for r in raw) / len(raw) if raw else 0.0,
            "context_hit_rate": sum(1 for r in raw if r.get("has_context")) / len(raw) if raw else 0.0,
            **compute_statistics(all_f1),
        }
        return {"overall": overall, "per_category": per_category}

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

    # ── LoCoMo ─────────────────────────────────────────────────────

    def _ingest_locomo(self, budget: int) -> None:
        if not self.locomo or budget <= 0:
            return
        conversations = self.locomo if isinstance(self.locomo, list) else self.locomo.get("data", [])
        for conv_idx, conv in enumerate(conversations):
            conversation_dict = conv.get("conversation", {})
            if isinstance(conversation_dict, dict):
                # Build session date map: "session_1" -> "8:56 pm on 20 July, 2023"
                session_dates: dict[str, str] = {}
                for key, value in conversation_dict.items():
                    if key.endswith("_date_time") and isinstance(value, str):
                        session_dates[key.replace("_date_time", "")] = value
                # Ingest each session's turns with date context
                for key, value in conversation_dict.items():
                    if key.startswith("session_") and not key.endswith("_date_time") and isinstance(value, list):
                        session_date = session_dates.get(key, "")
                        date_prefix = f"[Date: {session_date}] " if session_date else ""
                        for turn in value:
                            if isinstance(turn, dict):
                                content = f"{date_prefix}{turn.get('speaker', 'User')}: {turn.get('text', '')}"
                                self.client.create(content, tags=["locomo", f"conv-{conv_idx}"])
            elif isinstance(conversation_dict, list):
                for turn in conversation_dict:
                    if isinstance(turn, dict):
                        content = f"{turn.get('speaker', 'User')}: {turn.get('text', '')}"
                        self.client.create(content, tags=["locomo", f"conv-{conv_idx}"])

    def _query_locomo(self, budget: int) -> list[dict[str, Any]]:
        if not self.locomo or budget <= 0:
            return []
        conversations = self.locomo if isinstance(self.locomo, list) else self.locomo.get("data", [])
        results = []
        q_count = 0
        for conv in conversations:
            if q_count >= budget:
                break
            qa_list = conv.get("qa", [])
            for q in qa_list:
                if q_count >= budget:
                    break
                question = str(q.get("question", ""))
                answer = str(q.get("answer", "")) if q.get("answer") is not None else ""
                category = self._map_locomo_category(q.get("category", 0))

                # Pass category as intent hint + locomo tag filter for retrieval precision
                resp = self.client.search(question, limit=15, intent=category, require_tags=["locomo"])
                hits = resp.get("results", [])
                context = "\n".join(h.get("snippet", "") for h in hits[:10])
                has_context = bool(context.strip())

                # Use intent-specific prompt based on LoCoMo category
                prompt_template = CATEGORY_PROMPTS.get(category, READER_PROMPT)
                prompt = prompt_template.format(context=context, question=question)
                t_online = time.perf_counter()
                # CoT prompts need more tokens for reasoning before answer
                max_tok = 192 if category in ("multi_hop", "temporal") else 128
                raw_pred = self.llm.chat([{"role": "user", "content": prompt}], max_tokens=max_tok)
                # Extract answer after "Answer:" marker (HippoRAG-style)
                pred = _extract_answer(raw_pred)
                # Post-process temporal answers to reject relative time expressions
                if category == "temporal":
                    pred = _sanitize_temporal_answer(pred)
                online_lat = (time.perf_counter() - t_online) * 1000

                score, raw_judge = self.judge.evaluate_scored(question, answer, pred)
                results.append({
                    "dataset": "locomo",
                    "category": category,
                    "question": question,
                    "expected": answer,
                    "predicted": pred,
                    "f1": token_level_f1(pred, answer),
                    "bleu1": bleu1(pred, answer),
                    "llm_score": score,
                    "has_context": has_context,
                    "latency_online_ms": online_lat,
                })
                q_count += 1
        return results

    # ── LongMemEval ────────────────────────────────────────────────

    def _ingest_longmemeval(self, budget: int) -> None:
        if not self.longmemeval or budget <= 0:
            return
        data = self.longmemeval if isinstance(self.longmemeval, list) else []
        for item in data[:budget]:
            sessions = item.get("haystack_sessions", [])
            for session in sessions:
                if isinstance(session, list):
                    text = "\n".join(
                        f"{t.get('role','?')}: {t.get('content','')}"
                        for t in session if isinstance(t, dict)
                    )
                    if text.strip():
                        self.client.create(text, tags=["longmemeval"])

    def _query_longmemeval(self, budget: int) -> list[dict[str, Any]]:
        if not self.longmemeval or budget <= 0:
            return []
        data = self.longmemeval if isinstance(self.longmemeval, list) else []
        results = []
        for item in data[:budget]:
            question = str(item.get("question", ""))
            answer = str(item.get("answer", "")) if item.get("answer") is not None else ""
            category = item.get("question_type", "unknown")

            resp = self.client.search(question, limit=15, require_tags=["longmemeval"])
            hits = resp.get("results", [])
            context = "\n".join(h.get("snippet", "") for h in hits[:10])

            # Use category-specific prompt if available, otherwise generic
            prompt_template = CATEGORY_PROMPTS.get(category, READER_PROMPT)
            prompt = prompt_template.format(context=context, question=question)
            raw_pred = self.llm.chat([{"role": "user", "content": prompt}], max_tokens=128)
            pred = _extract_answer(raw_pred)
            score, _ = self.judge.evaluate_scored(question, answer, pred)
            results.append({
                "dataset": "longmemeval",
                "category": category,
                "question": question,
                "expected": answer,
                "predicted": pred,
                "f1": token_level_f1(pred, answer),
                "bleu1": bleu1(pred, answer),
                "llm_score": score,
                "has_context": bool(context.strip()),
            })
        return results

    def _map_locomo_category(self, cat: int) -> str:
        # LoCoMo category mapping (verified against v45 baseline):
        # 1=single_hop(282), 2=temporal(321), 3=multi_hop(96), 4=open_domain(841), 5=adversarial(446)
        mapping = {1: "single_hop", 2: "temporal", 3: "multi_hop", 4: "open_domain", 5: "adversarial"}
        return mapping.get(cat, "unknown")
