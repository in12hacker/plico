"""LLM provider abstraction — OpenAI-compatible chat/completion."""

from __future__ import annotations

import os
from typing import Any, Protocol

import requests


class LlmProvider(Protocol):
    """Abstract LLM provider."""

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str:
        ...

    def is_available(self) -> bool:
        ...


class OpenAiCompatibleLlm:
    """OpenAI-compatible LLM (llama.cpp server, vLLM, etc.)."""

    def __init__(
        self,
        api_base: str | None = None,
        model: str | None = None,
        api_key: str = "sk-no-key",
        timeout: float = 120.0,
    ):
        self.api_base = (api_base or os.environ.get("OPENAI_API_BASE", "http://127.0.0.1:8080/v1")).rstrip("/")
        self.model = model or os.environ.get("LLM_MODEL", "default")
        self.api_key = api_key
        self.timeout = timeout

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str:
        url = f"{self.api_base}/chat/completions"
        payload = {
            "model": self.model,
            "messages": messages,
            "temperature": kwargs.get("temperature", 0.0),
            "max_tokens": kwargs.get("max_tokens", 512),
        }
        if "top_p" in kwargs:
            payload["top_p"] = kwargs["top_p"]
        headers = {"Authorization": f"Bearer {self.api_key}", "Content-Type": "application/json"}
        resp = requests.post(url, json=payload, headers=headers, timeout=self.timeout)
        resp.raise_for_status()
        return resp.json()["choices"][0]["message"]["content"].strip()

    def is_available(self) -> bool:
        try:
            r = requests.get(f"{self.api_base}/models", timeout=5)
            return r.status_code == 200
        except Exception:
            return False


class LlamaCppLlm(OpenAiCompatibleLlm):
    """llama.cpp server — same as OpenAI-compatible but with different env defaults."""

    def __init__(
        self,
        url: str | None = None,
        model: str | None = None,
        timeout: float = 120.0,
    ):
        url = url or os.environ.get("LLAMA_URL", "http://127.0.0.1:8080/v1")
        model = model or os.environ.get("LLAMA_MODEL", "default")
        super().__init__(api_base=url, model=model, timeout=timeout)


class StubLlm:
    """Stub LLM for testing — returns fixed responses."""

    def __init__(self, response: str = "stub"):
        self.response = response

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str:
        return self.response

    def is_available(self) -> bool:
        return True


def default_llm() -> LlmProvider:
    """Factory — returns the best available LLM provider."""
    backend = os.environ.get("LLM_BACKEND", "openai").lower()
    if backend == "llama":
        return LlamaCppLlm()
    return OpenAiCompatibleLlm()
