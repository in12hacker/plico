"""LLM-as-Judge on the exact, fail-closed DeepSeek judge role boundary."""

from __future__ import annotations

import time
import uuid
from dataclasses import dataclass
from decimal import Decimal
from typing import Any

from plico_benchmarks.core.llm import LlmAttemptEvidence, LlmProvider, OpenAiCompatibleLlm
from plico_benchmarks.core.llm_roles import LlmRole

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

# RAGAS-style proxy prompts (0-10, normalized to 0.0-1.0).
# These prompts do not invoke the official ``ragas`` package or reproduce its
# claim decomposition/ranking algorithms. Results are proxy scores and are not
# directly comparable to published RAGAS measurements.

RAGAS_FAITHFULNESS_PROMPT = """Rate how faithful the answer is to the given context (0-10 scale).

Question: {question}
Context: {context}
Answer: {answer}

Scoring:
0 = Answer contains claims completely unsupported by context
5 = Answer is partially grounded but has some unsupported claims
10 = Every claim in the answer is directly supported by the context

Reply with ONLY a single integer 0-10."""

RAGAS_ANSWER_RELEVANCY_PROMPT = """Rate how relevant the answer is to the question
asked (0-10 scale).

Question: {question}
Context: {context}
Answer: {answer}

Scoring:
0 = Answer is completely irrelevant to the question
5 = Answer is partially relevant but misses key aspects
10 = Answer directly and completely addresses the question

Reply with ONLY a single integer 0-10."""

RAGAS_CONTEXT_PRECISION_PROMPT = """Rate how precisely the context addresses the
question (0-10 scale).

Question: {question}
Context: {context}

Scoring:
0 = Context is completely irrelevant to the question
5 = Context has some relevant information mixed with irrelevant content
10 = All context items are directly relevant and well-ranked for the question

Reply with ONLY a single integer 0-10."""

RAGAS_CONTEXT_RECALL_PROMPT = """Rate how well the context covers the ground truth
answer (0-10 scale).

Context: {context}
Ground truth: {ground_truth}

Scoring:
0 = Context contains none of the information needed for the ground truth
5 = Context covers about half of the ground truth claims
10 = Context fully covers all claims in the ground truth

Reply with ONLY a single integer 0-10."""


def _parse_ragas_score(raw: str) -> float:
    """Parse one canonical integer 0-10, returning 0.0-1.0."""
    normalized = raw.strip()
    if normalized not in {str(value) for value in range(11)}:
        raise ValueError("judge returned a noncanonical RAGAS proxy score")
    return int(normalized) / 10.0


def _parse_scored_score(raw: str) -> int:
    normalized = raw.strip()
    if normalized not in {"1", "2", "3", "4", "5"}:
        raise ValueError("judge returned a noncanonical 1-5 score")
    return int(normalized)


@dataclass
class JudgeResult:
    correct: bool
    raw_response: str
    latency_ms: float = 0.0
    attempt_evidence: tuple[LlmAttemptEvidence, ...] = ()
    usd_accounted: str = "0"
    role_request_id: str | None = None
    sample_id: str | None = None


@dataclass(frozen=True)
class ScoredJudgeResult:
    score: int
    raw_response: str
    attempt_evidence: tuple[LlmAttemptEvidence, ...] = ()
    usd_accounted: str = "0"
    role_request_id: str | None = None
    sample_id: str | None = None

    def __iter__(self):
        """Preserve the historical ``score, raw = result`` call interface."""
        yield self.score
        yield self.raw_response


class RagasJudgeResult(dict[str, float]):
    """Mapping-compatible proxy scores with role request/cost evidence."""

    def __init__(
        self,
        scores: dict[str, float],
        *,
        attempt_evidence: tuple[LlmAttemptEvidence, ...],
        usd_accounted: str,
        role_request_ids: tuple[str, ...],
        sample_id: str | None,
    ):
        super().__init__(scores)
        self.attempt_evidence = attempt_evidence
        self.usd_accounted = usd_accounted
        self.role_request_ids = role_request_ids
        self.sample_id = sample_id


class _JudgeResponseValidationError(RuntimeError):
    pass


class Judge:
    """DeepSeek-role judge with paid attempts owned by the LLM boundary."""

    def __init__(
        self,
        llm: LlmProvider | None = None,
        max_tokens: int = 32,
        retries: int | None = None,
    ):
        if retries not in {None, 1}:
            raise ValueError("paid retry count is owned by the exact LLM role configuration")
        self.llm = llm or self._from_env()
        self.max_tokens = max_tokens

    @classmethod
    def _from_env(cls) -> LlmProvider:
        return OpenAiCompatibleLlm(role=LlmRole.JUDGE)

    def _evidence_watermark(self) -> int:
        attempts = getattr(self.llm, "attempts", None)
        if not callable(attempts):
            return 0
        current = attempts()
        return current[-1].attempt_sequence if current else 0

    def _evidence_since(
        self, watermark: int, role_request_id: str
    ) -> tuple[LlmAttemptEvidence, ...]:
        evidence_since = getattr(self.llm, "evidence_since", None)
        return (
            evidence_since(watermark, role_request_id=role_request_id)
            if callable(evidence_since)
            else ()
        )

    def _chat_validated(
        self,
        messages: list[dict[str, str]],
        validator: Any,
        **kwargs: Any,
    ) -> str:
        paid_boundary = getattr(self.llm, "chat_validated", None)
        if callable(paid_boundary):
            return paid_boundary(messages, validator, **kwargs)
        raw = self.llm.chat(messages, **kwargs)
        if not validator(raw):
            raise _JudgeResponseValidationError("offline judge response failed validation")
        return raw

    def evaluate(
        self,
        question: str,
        expected: str,
        actual: str,
        custom_prompt: str | None = None,
        *,
        sample_id: str | None = None,
        request_id: str | None = None,
    ) -> JudgeResult:
        prompt = (custom_prompt or DEFAULT_PROMPT_TEMPLATE).format(
            question=question, expected=expected, actual=actual
        )
        role_request_id = request_id or str(uuid.uuid4())
        watermark = self._evidence_watermark()
        start = time.monotonic()
        try:
            raw = self._chat_validated(
                [{"role": "user", "content": prompt}],
                lambda value: value.strip().lower() in {"correct", "incorrect"},
                max_tokens=self.max_tokens,
                request_id=role_request_id,
                sample_id=sample_id,
            )
        except Exception as error:
            raise RuntimeError("judge evaluation failed") from error
        latency_ms = (time.monotonic() - start) * 1000
        evidence = self._evidence_since(watermark, role_request_id)
        usd = sum((Decimal(item.usd_accounted) for item in evidence), Decimal(0))
        return JudgeResult(
            correct=raw.strip().lower() == "correct",
            raw_response=raw,
            latency_ms=latency_ms,
            attempt_evidence=evidence,
            usd_accounted=format(usd, "f"),
            role_request_id=role_request_id,
            sample_id=sample_id,
        )

    def evaluate_scored(
        self,
        question: str,
        expected: str,
        actual: str,
        *,
        sample_id: str | None = None,
        request_id: str | None = None,
    ) -> ScoredJudgeResult:
        """Evaluate with the suite's explicit 1-5 judge protocol."""
        prompt = SCORED_PROMPT_TEMPLATE.format(question=question, expected=expected, actual=actual)
        role_request_id = request_id or str(uuid.uuid4())
        watermark = self._evidence_watermark()
        try:
            raw = self._chat_validated(
                [{"role": "user", "content": prompt}],
                lambda value: _is_canonical_scored(value),
                max_tokens=4,
                request_id=role_request_id,
                sample_id=sample_id,
            )
        except _JudgeResponseValidationError as error:
            raise RuntimeError("scored judge returned no 1-5 score") from error
        except Exception as error:
            raise RuntimeError("scored judge evaluation failed") from error
        evidence = self._evidence_since(watermark, role_request_id)
        usd = sum((Decimal(item.usd_accounted) for item in evidence), Decimal(0))
        return ScoredJudgeResult(
            score=_parse_scored_score(raw),
            raw_response=raw,
            attempt_evidence=evidence,
            usd_accounted=format(usd, "f"),
            role_request_id=role_request_id,
            sample_id=sample_id,
        )

    def is_available(self) -> bool:
        return self.llm.is_available()

    def describe(self) -> str:
        return f"Judge(model={getattr(self.llm, 'model', 'unknown')})"

    # ── RAGAS evaluation ────────────────────────────────────────────

    def evaluate_ragas_style_proxy(
        self,
        question: str,
        answer: str,
        context: str,
        ground_truth: str | None = None,
        *,
        sample_id: str | None = None,
        request_id: str | None = None,
    ) -> RagasJudgeResult:
        """Evaluate four RAGAS-inspired metrics with a single LLM judge.

        This is a lightweight proxy, not the official RAGAS implementation.
        Each returned value is in 0.0-1.0.
        """
        results = {}
        evidence_watermark = self._evidence_watermark()
        request_prefix = request_id or str(uuid.uuid4())
        role_request_ids: list[str] = []
        prompts = [
            (
                "faithfulness",
                RAGAS_FAITHFULNESS_PROMPT.format(question=question, answer=answer, context=context),
            ),
            (
                "answer_relevancy",
                RAGAS_ANSWER_RELEVANCY_PROMPT.format(
                    question=question, answer=answer, context=context
                ),
            ),
            (
                "context_precision",
                RAGAS_CONTEXT_PRECISION_PROMPT.format(question=question, context=context),
            ),
        ]
        if ground_truth:
            prompts.append(
                (
                    "context_recall",
                    RAGAS_CONTEXT_RECALL_PROMPT.format(context=context, ground_truth=ground_truth),
                )
            )
        for metric_name, prompt in prompts:
            metric_request_id = f"{request_prefix}:{metric_name}"
            role_request_ids.append(metric_request_id)
            results[metric_name] = self._score_ragas(
                prompt,
                sample_id=sample_id,
                request_id=metric_request_id,
            )
        if not ground_truth:
            results["context_recall"] = 0.0
        all_evidence = tuple(
            evidence
            for request in role_request_ids
            for evidence in self._evidence_since(evidence_watermark, request)
        )
        usd = sum((Decimal(item.usd_accounted) for item in all_evidence), Decimal(0))
        return RagasJudgeResult(
            results,
            attempt_evidence=all_evidence,
            usd_accounted=format(usd, "f"),
            role_request_ids=tuple(role_request_ids),
            sample_id=sample_id,
        )

    def _score_ragas(
        self,
        prompt: str,
        *,
        sample_id: str | None,
        request_id: str,
    ) -> float:
        """Score one RAGAS-style proxy (0-10, normalized to 0.0-1.0)."""
        try:
            raw = self._chat_validated(
                [{"role": "user", "content": prompt}],
                lambda value: _is_canonical_ragas(value),
                max_tokens=4,
                request_id=request_id,
                sample_id=sample_id,
            )
            return _parse_ragas_score(raw)
        except Exception as error:
            raise RuntimeError("RAGAS-style judge evaluation failed") from error


def _is_canonical_scored(raw: str) -> bool:
    try:
        _parse_scored_score(raw)
    except ValueError:
        return False
    return True


def _is_canonical_ragas(raw: str) -> bool:
    try:
        _parse_ragas_score(raw)
    except ValueError:
        return False
    return True
