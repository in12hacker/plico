#!/usr/bin/env python3
"""
Configurable Judge Interface for Plico Benchmarks

Abstracts the LLM judge used for evaluating benchmark answers.
Supports:
  - Local llama-server (default, same as inference server)
  - External OpenAI-compatible API (for higher-quality evaluation)

Environment Variables:
  PLICO_JUDGE_API_BASE  — Judge API base URL (default: same as LLM_URL or http://127.0.0.1:8080)
  PLICO_JUDGE_MODEL     — Judge model name (default: same as LLM_MODEL or "default")
  PLICO_JUDGE_MAX_TOKENS — Max tokens for judge response (default: 32)

Usage:
    from judge import Judge

    judge = Judge.from_env()
    result = judge.evaluate(question, expected, actual)
    # result.correct: bool
    # result.raw_response: str
"""

import os
from dataclasses import dataclass
from typing import Optional

import requests


@dataclass
class JudgeResult:
    """Result of a single judge evaluation."""
    correct: bool
    raw_response: str
    latency_ms: float = 0.0


JUDGE_PROMPT_TEMPLATE = """You are evaluating whether an AI assistant's answer is correct.
Question: {question}
Expected answer: {expected}
AI answer: {actual}

Is the AI answer correct or essentially equivalent to the expected answer?
Reply with ONLY "correct" or "incorrect"."""


class Judge:
    """Configurable LLM judge for benchmark evaluation."""

    def __init__(
        self,
        api_base: str,
        model: str,
        max_tokens: int = 32,
        timeout: int = 30,
    ):
        self.api_base = api_base.rstrip("/")
        self.model = model
        self.max_tokens = max_tokens
        self.timeout = timeout

    @classmethod
    def from_env(cls) -> "Judge":
        """Create a Judge from environment variables, with sensible defaults."""
        default_base = os.environ.get("LLM_URL", "http://127.0.0.1:8080")
        default_model = os.environ.get("LLM_MODEL", "default")

        api_base = os.environ.get("PLICO_JUDGE_API_BASE", default_base)
        model = os.environ.get("PLICO_JUDGE_MODEL", default_model)
        max_tokens = int(os.environ.get("PLICO_JUDGE_MAX_TOKENS", "32"))

        return cls(api_base=api_base, model=model, max_tokens=max_tokens)

    def evaluate(
        self,
        question: str,
        expected: str,
        actual: str,
        custom_prompt: Optional[str] = None,
    ) -> JudgeResult:
        """Evaluate whether the actual answer matches the expected answer.

        Args:
            question: The benchmark question
            expected: The ground-truth expected answer
            actual: The AI-generated answer to evaluate
            custom_prompt: Optional custom prompt template (must contain {question}, {expected}, {actual})

        Returns:
            JudgeResult with correctness determination
        """
        import time

        if custom_prompt:
            prompt = custom_prompt.format(
                question=question, expected=expected, actual=actual
            )
        else:
            prompt = JUDGE_PROMPT_TEMPLATE.format(
                question=question, expected=expected, actual=actual
            )

        start = time.monotonic()
        raw = self._call_llm(prompt)
        latency_ms = (time.monotonic() - start) * 1000

        correct = "correct" in raw.lower() and "incorrect" not in raw.lower()
        return JudgeResult(correct=correct, raw_response=raw, latency_ms=latency_ms)

    def evaluate_batch(
        self,
        items: list[dict],
        custom_prompt: Optional[str] = None,
    ) -> list[JudgeResult]:
        """Evaluate multiple items sequentially.

        Args:
            items: List of dicts with keys: question, expected, actual
            custom_prompt: Optional custom prompt template

        Returns:
            List of JudgeResults in the same order
        """
        return [
            self.evaluate(
                item["question"], item["expected"], item["actual"], custom_prompt
            )
            for item in items
        ]

    def is_available(self) -> bool:
        """Check if the judge LLM is reachable."""
        try:
            resp = requests.get(
                f"{self.api_base}/v1/models", timeout=5
            )
            return resp.status_code == 200
        except Exception:
            return False

    def describe(self) -> str:
        """Human-readable description of the judge configuration."""
        return f"Judge(api={self.api_base}, model={self.model}, max_tokens={self.max_tokens})"

    def _call_llm(self, prompt: str) -> str:
        try:
            resp = requests.post(
                f"{self.api_base}/v1/chat/completions",
                json={
                    "model": self.model,
                    "messages": [{"role": "user", "content": prompt}],
                    "max_tokens": self.max_tokens,
                    "temperature": 0.0,
                },
                timeout=self.timeout,
            )
            resp.raise_for_status()
            return resp.json()["choices"][0]["message"]["content"].strip()
        except Exception as e:
            return f"[JUDGE_ERROR: {e}]"


if __name__ == "__main__":
    judge = Judge.from_env()
    print(f"Judge config: {judge.describe()}")
    print(f"Available: {judge.is_available()}")

    if judge.is_available():
        result = judge.evaluate(
            question="What is the capital of France?",
            expected="Paris",
            actual="The capital of France is Paris.",
        )
        print(f"Test evaluation: correct={result.correct}, response='{result.raw_response}', latency={result.latency_ms:.0f}ms")
