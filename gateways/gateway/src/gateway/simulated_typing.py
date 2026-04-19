"""Simulated typing — paragraph-aware message splitting with per-chunk delays.

This module is intentionally *channel-agnostic*: it has no dependency on
discord.py, asyncio, or any I/O.  Given a full agent reply and a
SimulatedTypingConfig, it returns an ordered list of TypingChunk values
describing what to send and how long the caller should "type" before
sending each one.

Callers (e.g. the Discord gateway) are responsible for actually sleeping,
toggling a typing indicator, and dispatching the messages.

Heuristics:

* Paragraphs are separated by two or more consecutive newlines.  Single
  newlines stay inside the same paragraph (so wrapped prose and bullet
  lists without blank lines are preserved as one message).
* Triple-backtick code fences are atomic — internal blank lines never
  cause a split inside a fenced block, so syntax highlighting survives
  intact.
* Per-paragraph delay is `words * 60 / wpm`, clamped to
  `[min_delay_s, max_delay_s]`.  Word count is `len(paragraph.split())`,
  which is good enough for a feel-feature.
* When simulated typing is disabled the entire message is returned as a
  single chunk with `delay_s == 0.0`, so callers can use a uniform code
  path.
"""

from __future__ import annotations

from dataclasses import dataclass

DEFAULT_WPM = 220
DEFAULT_MIN_DELAY_S = 0.4
DEFAULT_MAX_DELAY_S = 12.0


@dataclass(frozen=True, slots=True)
class SimulatedTypingConfig:
    """Configuration for `plan_simulated_typing`."""

    enabled: bool
    wpm: int = DEFAULT_WPM
    min_delay_s: float = DEFAULT_MIN_DELAY_S
    max_delay_s: float = DEFAULT_MAX_DELAY_S


@dataclass(frozen=True, slots=True)
class TypingChunk:
    """A single piece of text the caller should send, after a typing delay."""

    delay_s: float
    content: str


def plan_simulated_typing(
    message: str,
    config: SimulatedTypingConfig,
) -> list[TypingChunk]:
    """Plan how to deliver `message` as a sequence of typed chunks.

    Returns an empty list if `message` is empty or whitespace-only.
    Returns a single zero-delay chunk if `config.enabled` is False.
    """
    if not message.strip():
        return []

    if not config.enabled:
        return [TypingChunk(delay_s=0.0, content=message)]

    paragraphs = _split_into_paragraphs(message)
    return [
        TypingChunk(
            delay_s=_typing_delay_for(para, config),
            content=para,
        )
        for para in paragraphs
    ]


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _split_into_paragraphs(message: str) -> list[str]:
    """Split `message` into paragraphs, respecting code-fence boundaries.

    A paragraph break is two or more consecutive newlines (ignoring
    horizontal whitespace on the blank line).  Inside a triple-backtick
    code fence, blank lines do NOT cause a split.
    """
    lines = message.splitlines()
    paragraphs: list[str] = []
    current: list[str] = []
    in_fence = False

    def flush() -> None:
        if not current:
            return
        # Trim trailing blank lines from the paragraph; leading blanks were
        # already filtered by the split logic.
        text = "\n".join(current).rstrip()
        if text:
            paragraphs.append(text)
        current.clear()

    for line in lines:
        if _is_code_fence(line):
            in_fence = not in_fence
            current.append(line)
            continue

        if not in_fence and _is_blank(line):
            # Blank line outside a fence: flush the current paragraph (if
            # any).  Consecutive blanks just keep flushing an empty buffer.
            flush()
            continue

        current.append(line)

    # Close any unterminated fence by flushing what's left.
    flush()
    return paragraphs


def _is_blank(line: str) -> bool:
    return line.strip() == ""


def _is_code_fence(line: str) -> bool:
    """Return True if the line opens or closes a code fence.

    A line is a code fence if it starts with triple-backtick (after
    optional indent).  We don't try to match opening/closing fences by
    language tag — any triple-backtick line toggles fence state, which
    matches how Markdown renderers (and Discord) behave in practice.
    """
    return line.lstrip().startswith("```")


def _typing_delay_for(paragraph: str, config: SimulatedTypingConfig) -> float:
    """Compute the typing delay for one paragraph, clamped to config bounds."""
    words = max(1, len(paragraph.split()))
    seconds = words * 60.0 / max(1, config.wpm)
    return min(config.max_delay_s, max(config.min_delay_s, seconds))
