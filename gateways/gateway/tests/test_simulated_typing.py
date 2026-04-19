"""Tests for the channel-agnostic simulated typing planner."""

from __future__ import annotations

import math

from gateway.simulated_typing import (
    DEFAULT_MAX_DELAY_S,
    DEFAULT_MIN_DELAY_S,
    SimulatedTypingConfig,
    plan_simulated_typing,
)


def _approx(actual: float, expected: float) -> bool:
    return math.isclose(actual, expected, rel_tol=1e-6, abs_tol=1e-9)


# --- enabled / disabled / empty ---------------------------------------------


def test_disabled_returns_single_zero_delay_chunk() -> None:
    cfg = SimulatedTypingConfig(enabled=False)
    out = plan_simulated_typing("hello\n\nworld", cfg)
    assert len(out) == 1
    assert out[0].delay_s == 0.0
    assert out[0].content == "hello\n\nworld"


def test_empty_message_returns_no_chunks() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    assert plan_simulated_typing("", cfg) == []
    assert plan_simulated_typing("   \n\n  ", cfg) == []


# --- splitting heuristics ---------------------------------------------------


def test_simple_paragraphs_split_on_blank_lines() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    out = plan_simulated_typing("first\n\nsecond\n\nthird", cfg)
    assert [c.content for c in out] == ["first", "second", "third"]


def test_single_newline_does_not_split() -> None:
    """Wrapped prose (single newlines) stays as one paragraph."""
    cfg = SimulatedTypingConfig(enabled=True)
    out = plan_simulated_typing("line one\nline two\nline three", cfg)
    assert [c.content for c in out] == ["line one\nline two\nline three"]


def test_bullet_list_without_blank_lines_stays_one_paragraph() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    text = "- alpha\n- beta\n- gamma"
    out = plan_simulated_typing(text, cfg)
    assert [c.content for c in out] == [text]


def test_extra_blank_lines_are_collapsed() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    out = plan_simulated_typing("a\n\n\n\nb", cfg)
    assert [c.content for c in out] == ["a", "b"]


def test_blank_line_with_whitespace_still_counts_as_break() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    out = plan_simulated_typing("a\n   \nb", cfg)
    assert [c.content for c in out] == ["a", "b"]


def test_code_fence_is_atomic_across_blank_lines() -> None:
    """A blank line inside ``` ... ``` must NOT cause a split."""
    cfg = SimulatedTypingConfig(enabled=True)
    text = (
        "intro paragraph\n"
        "\n"
        "```python\n"
        "def f():\n"
        "\n"  # blank line inside the fence
        "    return 1\n"
        "```\n"
        "\n"
        "outro paragraph"
    )
    out = plan_simulated_typing(text, cfg)
    assert len(out) == 3
    assert out[0].content == "intro paragraph"
    assert out[1].content.startswith("```python")
    assert out[1].content.endswith("```")
    assert "def f():" in out[1].content
    assert "    return 1" in out[1].content
    assert out[2].content == "outro paragraph"


def test_unterminated_code_fence_is_still_flushed() -> None:
    cfg = SimulatedTypingConfig(enabled=True)
    out = plan_simulated_typing("```\nstuck open", cfg)
    assert len(out) == 1
    assert out[0].content.startswith("```")


# --- timing math ------------------------------------------------------------


def test_delay_scales_with_word_count() -> None:
    cfg = SimulatedTypingConfig(
        enabled=True,
        wpm=60,
        min_delay_s=0.0,
        max_delay_s=1000.0,
    )
    # 60 wpm = 1 word per second, so N words ≈ N seconds.
    out = plan_simulated_typing("one two three four five", cfg)
    assert len(out) == 1
    assert _approx(out[0].delay_s, 5.0)


def test_delay_clamped_to_min() -> None:
    cfg = SimulatedTypingConfig(enabled=True, wpm=10000, min_delay_s=0.5)
    out = plan_simulated_typing("hi", cfg)
    assert _approx(out[0].delay_s, 0.5)


def test_delay_clamped_to_max() -> None:
    cfg = SimulatedTypingConfig(
        enabled=True,
        wpm=10,
        min_delay_s=0.0,
        max_delay_s=2.0,
    )
    out = plan_simulated_typing(" ".join(["word"] * 100), cfg)
    assert _approx(out[0].delay_s, 2.0)


def test_default_bounds_apply() -> None:
    cfg = SimulatedTypingConfig(enabled=True, wpm=220)
    # Empty-ish chunk hits min.
    out = plan_simulated_typing("ok", cfg)
    assert _approx(out[0].delay_s, DEFAULT_MIN_DELAY_S)
    # Huge paragraph hits max.
    huge = " ".join(["word"] * 10_000)
    out = plan_simulated_typing(huge, cfg)
    assert _approx(out[0].delay_s, DEFAULT_MAX_DELAY_S)


def test_zero_wpm_is_treated_as_one() -> None:
    """Defensive: a misconfigured wpm=0 must not cause a divide-by-zero."""
    cfg = SimulatedTypingConfig(enabled=True, wpm=0, max_delay_s=1e9)
    out = plan_simulated_typing("hello world", cfg)
    # 2 words / 1 wpm * 60 = 120s; just assert it didn't crash and is finite.
    assert out[0].delay_s > 0
