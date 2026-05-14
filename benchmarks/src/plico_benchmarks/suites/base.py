"""Suite utilities and shared helpers."""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.client import PlicoClient
from plico_benchmarks.core.harness import BaseSuite
from plico_benchmarks.core.judge import Judge
from plico_benchmarks.core.llm import default_llm


class SuiteBase(BaseSuite):
    """Extended base suite with common utilities."""

    def __init__(
        self,
        client: PlicoClient | None = None,
        host: str = "127.0.0.1",
        port: int = 7878,
        samples: int | None = None,
    ):
        super().__init__(client=client, host=host, port=port, samples=samples)
        self.llm = default_llm()
        self.judge = Judge()

    def wait_for_plico(self, timeout: float = 30.0) -> None:
        """Poll health until plicod is ready."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                resp = self.client.health()
                if resp.get("ok"):
                    return
            except Exception:
                pass
            time.sleep(1.0)
        raise TimeoutError("plicod not ready")

    def llm_answer(self, prompt: str, max_tokens: int = 64) -> str:
        """Get answer from LLM."""
        return self.llm.chat([{"role": "user", "content": prompt}], max_tokens=max_tokens)
