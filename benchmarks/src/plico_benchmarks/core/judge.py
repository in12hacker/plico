"""Configurable LLM-as-Judge with retry, concurrency, and multiple providers."""

from __future__ import annotations

import os
import time
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from typing import Any

from plico_benchmarks.core.llm import LlmProvider, OpenAiCompatibleLlm

DEFAULT_PROMPT_TEMPLATE = """You are evaluating whether an AI assistant's answer is correct.
Question: {question}
Expected answer: {expected}
AI answer: {actual}

Is the AI answer correct or essentially equivalent to the expected answer?
Reply with ONLY "correct" or "incorrect"."""

SCORED_PROMPT_TEMPLATE = """Rate the AI assistant's answer on a scale of 1-5.

Question: {question}
Expected answer: {expected}
AI answer: {actual}

Scoring:
1 = Completely wrong or "I don't know" when answer exists in context
2 = Mostly wrong, contains some relevant info but incorrect conclusion
3 = Partially correct, some key elements right but incomplete or imprecise
4 = Mostly correct, minor wording differences or slight imprecision
5 = Correct and equivalent to expected answer (synonyms, paraphrases OK)

Reply with ONLY a single digit 1-5."""

# ── RAGAS prompt templates (0-10 integer scale, normalized to 0.0-1.0) ──

RAGAS_FAITHFULNESS_PROMPT = """Rate how faithful the answer is to the given context (0-10 scale).

Question: {question}
Context: {context}
Answer: {answer}

Scoring:
0 = Answer contains claims completely unsupported by context
5 = Answer is partially grounded but has some unsupported claims
10 = Every claim in the answer is directly supported by the context

Reply with ONLY a single integer 0-10."""

RAGAS_ANSWER_RELEVANCY_PROMPT = """Rate how relevant the answer is to the question asked (0-10 scale).

Question: {question}
Context: {context}
Answer: {answer}

Scoring:
0 = Answer is completely irrelevant to the question
5 = Answer is partially relevant but misses key aspects
10 = Answer directly and completely addresses the question

Reply with ONLY a single integer 0-10."""

RAGAS_CONTEXT_PRECISION_PROMPT = """Rate how precisely the context addresses the question (0-10 scale).

Question: {question}
Context: {context}

Scoring:
0 = Context is completely irrelevant to the question
5 = Context has some relevant information mixed with irrelevant content
10 = All context items are directly relevant and well-ranked for the question

Reply with ONLY a single integer 0-10."""

RAGAS_CONTEXT_RECALL_PROMPT = """Rate how well the context covers the ground truth answer (0-10 scale).

Context: {context}
Ground truth: {ground_truth}

Scoring:
0 = Context contains none of the information needed for the ground truth
5 = Context covers about half of the ground truth claims
10 = Context fully covers all claims in the ground truth

Reply with ONLY a single integer 0-10."""


def _parse_ragas_score(raw: str) -> float:
    """Parse RAGAS score from LLM response. Expects 0-10, returns 0.0-1.0."""
    import re
    match = re.search(r"\d+", raw.strip())
    if match:
        score = int(match.group())
        return min(score, 10) / 10.0
    return 0.0


@dataclass
class JudgeResult:
    correct: bool
    raw_response: str
    latency_ms: float = 0.0


class Judge:
    """LLM judge with batch evaluation support."""

    def __init__(
        self,
        llm: LlmProvider | None = None,
        max_tokens: int = 32,
        max_workers: int = 4,
        retries: int = 2,
    ):
        self.llm = llm or self._from_env()
        self.max_tokens = max_tokens
        self.max_workers = max_workers
        self.retries = retries

    @classmethod
    def _from_env(cls) -> LlmProvider:
        base = os.environ.get("PLICO_JUDGE_API_BASE")
        model = os.environ.get("PLICO_JUDGE_MODEL")
        if base or model:
            return OpenAiCompatibleLlm(api_base=base, model=model)
        # fallback to default LLM
        return OpenAiCompatibleLlm()

    def evaluate(
        self,
        question: str,
        expected: str,
        actual: str,
        custom_prompt: str | None = None,
    ) -> JudgeResult:
        prompt = (custom_prompt or DEFAULT_PROMPT_TEMPLATE).format(
            question=question, expected=expected, actual=actual
        )
        for attempt in range(self.retries):
            try:
                start = time.monotonic()
                raw = self.llm.chat(
                    [{"role": "user", "content": prompt}],
                    max_tokens=self.max_tokens,
                    temperature=0.0,
                )
                latency_ms = (time.monotonic() - start) * 1000
                correct = "correct" in raw.lower() and "incorrect" not in raw.lower()
                return JudgeResult(correct=correct, raw_response=raw, latency_ms=latency_ms)
            except Exception as e:
                if attempt == self.retries - 1:
                    return JudgeResult(
                        correct=False, raw_response=f"[JUDGE_ERROR: {e}]", latency_ms=0.0
                    )
                time.sleep(0.5 * (attempt + 1))
        return JudgeResult(correct=False, raw_response="[JUDGE_ERROR: unknown]", latency_ms=0.0)

    def evaluate_batch(
        self, items: list[dict[str, Any]], custom_prompt: str | None = None
    ) -> list[JudgeResult]:
        """Evaluate multiple items with thread-pool concurrency."""
        if self.max_workers <= 1:
            return [
                self.evaluate(
                    item["question"], item["expected"], item["actual"], custom_prompt
                )
                for item in items
            ]
        with ThreadPoolExecutor(max_workers=self.max_workers) as ex:
            futures = [
                ex.submit(
                    self.evaluate,
                    item["question"],
                    item["expected"],
                    item["actual"],
                    custom_prompt,
                )
                for item in items
            ]
            return [f.result() for f in futures]

    def evaluate_scored(
        self,
        question: str,
        expected: str,
        actual: str,
    ) -> tuple[int, str]:
        """Evaluate with 1-5 score scale (comparable to v45 baseline)."""
        prompt = SCORED_PROMPT_TEMPLATE.format(
            question=question, expected=expected, actual=actual
        )
        for attempt in range(self.retries):
            try:
                raw = self.llm.chat(
                    [{"role": "user", "content": prompt}],
                    max_tokens=4,
                    temperature=0.0,
                )
                # Extract digit from response
                for ch in raw.strip():
                    if ch in "12345":
                        return int(ch), raw
                return 1, raw  # fallback if no digit found
            except Exception as e:
                if attempt == self.retries - 1:
                    return 1, f"[JUDGE_ERROR: {e}]"
                time.sleep(0.5 * (attempt + 1))
        return 1, "[JUDGE_ERROR: unknown]"

    def is_available(self) -> bool:
        return self.llm.is_available()

    def describe(self) -> str:
        return f"Judge(model={getattr(self.llm, 'model', 'unknown')}, max_workers={self.max_workers})"

    # ── RAGAS evaluation ────────────────────────────────────────────

    def evaluate_ragas(
        self,
        question: str,
        answer: str,
        context: str,
        ground_truth: str | None = None,
    ) -> dict[str, float]:
        """Evaluate RAGAS metrics: faithfulness, answer_relevancy, context_precision, context_recall.

        Returns dict with keys: faithfulness, answer_relevancy, context_precision, context_recall.
        Each value is 0.0-1.0.
        """
        results = {}
        prompts = [
            ("faithfulness", RAGAS_FAITHFULNESS_PROMPT.format(question=question, answer=answer, context=context)),
            ("answer_relevancy", RAGAS_ANSWER_RELEVANCY_PROMPT.format(question=question, answer=answer, context=context)),
            ("context_precision", RAGAS_CONTEXT_PRECISION_PROMPT.format(question=question, context=context)),
        ]
        if ground_truth:
            prompts.append(
                ("context_recall", RAGAS_CONTEXT_RECALL_PROMPT.format(context=context, ground_truth=ground_truth))
            )
        for metric_name, prompt in prompts:
            results[metric_name] = self._score_ragas(prompt)
        if not ground_truth:
            results["context_recall"] = 0.0
        return results

    def _score_ragas(self, prompt: str) -> float:
        """Score a single RAGAS metric (0-10 scale, normalized to 0.0-1.0)."""
        for attempt in range(self.retries):
            try:
                raw = self.llm.chat(
                    [{"role": "user", "content": prompt}],
                    max_tokens=4,
                    temperature=0.0,
                )
                return _parse_ragas_score(raw)
            except Exception:
                if attempt == self.retries - 1:
                    return 0.0
                time.sleep(0.5 * (attempt + 1))
        return 0.0
