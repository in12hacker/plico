"""W0 W-05 fallback-estimator pins (wheels audit): estimate_tokens stays the
documented fallback when provider usage fields are absent; real usage fields
(e.g. Ollama prompt_eval_count/eval_count) are preferred by callers."""


from plico_benchmarks.core.metrics import estimate_tokens


def test_estimate_tokens_empty() -> None:
    assert estimate_tokens("") == 0


def test_estimate_tokens_ascii_fallback() -> None:
    # 8 ASCII characters -> max(1, 8 // 4) = 2
    assert estimate_tokens("abcdefgh") == 2


def test_estimate_tokens_cjk_fallback() -> None:
    # 4 CJK characters (~1 token each) + the max(1, .) floor on the empty
    # non-CJK remainder -> 5
    assert estimate_tokens("会议记录") == 5


def test_estimate_tokens_mixed_fallback() -> None:
    # 2 CJK + 4 ASCII -> 2 + max(1, 4 // 4) = 3
    assert estimate_tokens("会议abcd") == 3
