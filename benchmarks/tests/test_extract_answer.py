"""Tests for _extract_answer CoT extraction robustness (T3)."""

import json
import sys
from pathlib import Path

# Add src to path so we can import the module
sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.suites.conversational_qa import _extract_answer


def test_answer_marker():
    raw = "Thought: Let me think.\nAnswer: Paris"
    assert _extract_answer(raw) == "Paris"


def test_answer_marker_case_insensitive():
    raw = "Thought: reasoning\nanswer: lowercase marker"
    assert _extract_answer(raw) == "lowercase marker"


def test_final_answer_marker():
    raw = "Some reasoning here.\nFinal Answer: 42"
    assert _extract_answer(raw) == "42"


def test_a_marker():
    raw = "Reasoning...\nA: yes"
    assert _extract_answer(raw) == "yes"


def test_json_answer_key():
    raw = json.dumps({"answer": "Berlin"})
    assert _extract_answer(raw) == "Berlin"


def test_json_final_answer_key():
    raw = json.dumps({"final_answer": "Tokyo", "reasoning": "because"})
    assert _extract_answer(raw) == "Tokyo"


def test_json_result_key():
    raw = json.dumps({"result": "7"})
    assert _extract_answer(raw) == "7"


def test_plain_text_fallback():
    raw = "The answer is simply London"
    assert _extract_answer(raw) == "The answer is simply London"


def test_empty_string():
    assert _extract_answer("") == ""


def test_whitespace_only():
    assert _extract_answer("   \n  ") == ""


def test_answer_marker_with_extra_whitespace():
    raw = "Thought: x\nAnswer:   Madrid   "
    assert _extract_answer(raw) == "Madrid"


def test_multiple_answer_markers_uses_last():
    raw = "Answer: wrong\nMore reasoning\nAnswer: correct"
    assert _extract_answer(raw) == "correct"


def test_json_invalid_falls_through():
    raw = '{"answer": "broken json'
    # Should not raise, falls through to text parsing
    result = _extract_answer(raw)
    assert isinstance(result, str)


def test_no_marker_raw_text():
    raw = "Just a plain response without any markers"
    assert _extract_answer(raw) == "Just a plain response without any markers"
