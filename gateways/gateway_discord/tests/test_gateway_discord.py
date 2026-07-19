from __future__ import annotations

import asyncio
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, cast

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient
from gateway.protocol import GatewayConfig, GatewayStatus
from gateway.simulated_typing import SimulatedTypingConfig
from gateway_discord import DiscordGateway
from gateway_discord import discord_gateway as gw_mod
from gateway_discord.discord_gateway import _chunk  # type: ignore[reportPrivateUsage]

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

    import discord
    from gateway.client import ClientServiceClient


# --- _chunk pure-function tests --------------------------------------------


def test_chunk_returns_empty_list_for_empty_text() -> None:
    assert _chunk("", 100) == []
    assert _chunk("\n\n\n", 100) == []


def test_chunk_returns_empty_list_for_whitespace_only_text() -> None:
    # Discord rejects sends of pure-whitespace content with a 400 — make sure
    # _chunk strips it before it reaches the wire.
    assert _chunk("   ", 100) == []
    assert _chunk(" \t\n  \r\n ", 100) == []


def test_chunk_returns_single_chunk_when_under_limit() -> None:
    assert _chunk("hello world", 100) == ["hello world"]


def test_chunk_splits_on_paragraph_boundaries() -> None:
    text = "para one\n\npara two\n\npara three"
    chunks = _chunk(text, 12)
    assert chunks == ["para one", "para two", "para three"]


def test_chunk_splits_long_paragraph_on_lines() -> None:
    text = "line a\nline b\nline c"
    chunks = _chunk(text, 8)
    assert chunks == ["line a", "line b", "line c"]
    assert all(len(c) <= 8 for c in chunks)


def test_chunk_hard_splits_long_line() -> None:
    text = "a" * 25
    chunks = _chunk(text, 10)
    assert chunks == ["a" * 10, "a" * 10, "a" * 5]


def test_chunk_packs_multiple_paragraphs_when_they_fit() -> None:
    text = "aaa\n\nbbb\n\nccc"
    # Each paragraph is 3 chars; combined "aaa\n\nbbb" is 8 chars.
    chunks = _chunk(text, 8)
    assert chunks == ["aaa\n\nbbb", "ccc"]


def test_chunk_rejects_zero_max() -> None:
    with pytest.raises(ValueError, match="must be positive"):
        _chunk("x", 0)


# --- Discord stubs ----------------------------------------------------------


@dataclass
class FakeAuthor:
    id: int
    bot: bool = False


@dataclass
class FakeChannel:
    sent: list[str] = field(default_factory=list[str])
    typing_calls: int = 0
    typing_entered: int = 0
    typing_exited: int = 0
    # Snapshot of typing_active at the moment each send() was made.  Lets
    # tests assert "typing was no longer active when the reply was sent".
    typing_active_at_send: list[int] = field(default_factory=list[int])

    @property
    def typing_active(self) -> int:
        """Currently-open typing contexts (entered minus exited)."""
        return self.typing_entered - self.typing_exited

    async def send(self, content: str) -> object:
        self.typing_active_at_send.append(self.typing_active)
        self.sent.append(content)
        return FakeSentMessage(channel=self, content=content)

    def typing(self) -> object:
        self.typing_calls += 1

        @asynccontextmanager
        async def _cm() -> AsyncIterator[None]:
            self.typing_entered += 1
            try:
                yield
            finally:
                self.typing_exited += 1

        return _cm()


@dataclass
class FakeSentMessage:
    channel: FakeChannel
    content: str

    async def delete(self) -> object:
        if self.content in self.channel.sent:
            index = self.channel.sent.index(self.content)
            self.channel.sent.pop(index)
            self.channel.typing_active_at_send.pop(index)
        return None


@dataclass
class FakeMessage:
    author: FakeAuthor
    content: str
    channel: FakeChannel
    guild: object | None = None
    reactions: list[str] = field(default_factory=list[str])
    removed_reactions: list[str] = field(default_factory=list[str])
    add_reaction_error: BaseException | None = None

    async def add_reaction(self, emoji: str) -> object:
        if self.add_reaction_error is not None:
            raise self.add_reaction_error
        self.reactions.append(emoji)
        return None

    async def remove_reaction(self, emoji: str, member: object) -> object:
        del member
        self.removed_reactions.append(emoji)
        if emoji in self.reactions:
            self.reactions.remove(emoji)
        return None


@dataclass
class FakeClient:
    sessions_created: list[tuple[str, str | None]] = field(
        default_factory=list[tuple[str, str | None]],
    )
    sessions_reset: list[str] = field(default_factory=list[str])
    sent_messages: list[tuple[str, str]] = field(default_factory=list[tuple[str, str]])
    fail_send: bool = False
    next_session_id: int = 0
    reply: str = "hello back"
    tool_calls: list[dict[str, object]] = field(default_factory=list[dict[str, object]])
    stream_items: list[dict[str, object]] | None = None

    async def create_session(
        self,
        *,
        agent_id: str,
        channel_name: str | None = None,
    ) -> dict[str, object]:
        self.sessions_created.append((agent_id, channel_name))
        self.next_session_id += 1
        return {"session_id": f"sess-{self.next_session_id}"}

    async def reset_session(self, *, session_id: str) -> dict[str, object]:
        self.sessions_reset.append(session_id)
        return {"session_id": session_id}

    async def send_message(
        self,
        *,
        session_id: str,
        message: str,
    ) -> dict[str, object]:
        self.sent_messages.append((session_id, message))
        if self.fail_send:
            msg = "boom"
            raise RuntimeError(msg)
        assistant_message: dict[str, object] = {"content": self.reply}
        if self.tool_calls:
            assistant_message["tool_calls"] = self.tool_calls
        return {"assistant_message": assistant_message}

    def stream_message(
        self,
        *,
        session_id: str,
        message: str,
    ) -> AsyncIterator[dict[str, object]]:
        async def _iterator() -> AsyncIterator[dict[str, object]]:
            self.sent_messages.append((session_id, message))
            if self.fail_send:
                msg = "boom"
                raise RuntimeError(msg)
            if self.stream_items is not None:
                for item in self.stream_items:
                    yield item
                return
            assistant_message: dict[str, object] = {"content": self.reply}
            if self.tool_calls:
                assistant_message["tool_calls"] = self.tool_calls
            yield {
                "type": "final",
                "completed": True,
                "assistant_message": assistant_message,
            }

        return _iterator()

    async def delete_session(self, *, session_id: str) -> None:
        del session_id


def _make_config(client: FakeClient, **env_overrides: str) -> GatewayConfig:
    env = {
        "DISCORD_BOT_TOKEN": "fake-token",
        "DISCORD_OWNER_USER_ID": "111",
        "DISCORD_CHUNK_MAX_CHARS": "20",
    }
    env.update(env_overrides)
    return GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", client),
        env=env,
    )


def _ready_gateway(client: FakeClient, **env_overrides: str) -> DiscordGateway:
    """Build a DiscordGateway already in RUNNING state without touching discord.py."""
    gateway = DiscordGateway()
    config = _make_config(client, **env_overrides)
    # Manually wire what start() would have produced after on_ready fires.
    gateway._config = config  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    gateway._owner_id = int(config.env["DISCORD_OWNER_USER_ID"])  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    gateway._chunk_max = int(config.env["DISCORD_CHUNK_MAX_CHARS"])  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    sim_enabled = config.env.get("DISCORD_SIMULATED_TYPING_ENABLED", "false")
    sim_wpm = int(config.env.get("DISCORD_SIMULATED_TYPING_WPM", "220"))
    gateway._sim_typing_cfg = SimulatedTypingConfig(  # type: ignore[reportPrivateUsage]  # noqa: SLF001
        enabled=sim_enabled.strip().lower() in {"1", "true", "yes", "on"},
        wpm=sim_wpm,
    )
    gateway._status = GatewayStatus.RUNNING  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    # _swap_reaction needs self._client.user to identify which reaction to
    # remove; supply a minimal stub so reaction-swap tests can run.
    gateway._client = cast("discord.Client", _StubBotClient())  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    return gateway


@dataclass
class _StubBotClient:
    user: object = field(default_factory=object)


# --- DiscordGateway behaviour tests -----------------------------------------


@pytest.mark.asyncio
async def test_owner_dm_creates_session_and_replies() -> None:
    fake = FakeClient(reply="hello back")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi there",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert fake.sessions_created == [("agent-1", "discord:dm:111")]
    assert fake.sent_messages == [("sess-1", "hi there")]
    assert channel.sent == ["hello back"]
    # On success the EYES is removed and replaced with the CHECK MARK.
    assert msg.removed_reactions == ["\N{EYES}"]
    assert msg.reactions == ["\N{WHITE HEAVY CHECK MARK}"]
    # Typing indicator was active during the turn and is fully closed now.
    assert channel.typing_entered >= 1
    assert channel.typing_active == 0
    # Crucially, typing was already CLOSED before the reply was delivered.
    # Otherwise discord.py's background refresh task could re-assert typing
    # right after the reply lands, making the indicator briefly reappear.
    assert channel.typing_active_at_send == [0]
    assert gateway.status is GatewayStatus.RUNNING


@pytest.mark.asyncio
async def test_typing_indicator_stops_on_agent_failure() -> None:
    """If the agent call raises, the typing context must still unwind cleanly.

    This is the whole point of using channel.typing() as a context manager:
    the user must never see a stuck "agent is typing" indicator after a
    failed turn.
    """
    fake = FakeClient(fail_send=True)
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    # Typing was started, then properly stopped despite the agent failure.
    assert channel.typing_entered >= 1
    assert channel.typing_active == 0
    # Failure path drives the gateway to ERROR (per existing contract).
    assert gateway.status is GatewayStatus.ERROR


@pytest.mark.asyncio
async def test_typing_enter_failure_does_not_drop_turn() -> None:
    """If channel.typing() itself raises on enter, the turn must still run.

    Typing is purely cosmetic; a transient REST blip or missing Send
    Messages permission must not prevent the agent reply from being
    delivered.
    """
    fake = FakeClient(reply="ok")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()

    # Replace channel.typing() with one that raises on __aenter__.
    @asynccontextmanager
    async def _broken_typing() -> AsyncIterator[None]:
        msg = "typing not allowed"
        raise RuntimeError(msg)
        yield  # pragma: no cover - unreachable, satisfies asynccontextmanager

    channel.typing = _broken_typing  # type: ignore[method-assign]

    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    # Reply was still delivered despite typing failing on enter.
    assert fake.sent_messages == [("sess-1", "hi")]
    assert channel.sent == ["ok"]
    assert gateway.status is GatewayStatus.RUNNING


@pytest.mark.asyncio
async def test_reaction_failure_does_not_abort_reply() -> None:
    """Verify add_reaction failures don't abort the turn.

    If add_reaction raises (e.g. missing permission), the turn must still
    complete and the user must still receive the assistant reply.
    """
    fake = FakeClient(reply="ok")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
        add_reaction_error=RuntimeError("missing Add Reactions permission"),
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert fake.sent_messages == [("sess-1", "hi")]
    assert channel.sent == ["ok"]
    # No reaction ever stuck because add_reaction kept raising; gateway is
    # still healthy.
    assert msg.reactions == []
    assert gateway.status is GatewayStatus.RUNNING


@pytest.mark.asyncio
async def test_owner_dm_reuses_session_across_messages() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    for text in ("hi", "hello", "again"):
        msg = FakeMessage(
            author=FakeAuthor(id=111),
            content=text,
            channel=channel,
        )
        await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert len(fake.sessions_created) == 1
    assert [s for s, _ in fake.sent_messages] == ["sess-1", "sess-1", "sess-1"]


@pytest.mark.asyncio
async def test_owner_dm_streams_text_before_tool_call_in_order() -> None:
    fake = FakeClient(
        stream_items=[
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {
                            "type": "text",
                            "text": "Let me search that for you...",
                        },
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-1",
                        "title": "get_weather",
                        "rawInput": {"location": "LA"},
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "sunny and 72"},
                    },
                },
            },
            {
                "type": "final",
                "completed": True,
                "assistant_message": {
                    "content": "Let me search that for you...sunny and 72",
                },
            },
        ],
    )
    gateway = _ready_gateway(fake, DISCORD_CHUNK_MAX_CHARS="1900")
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="what's the weather in LA right now?",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert channel.sent == [
        "Let me search that for you...",
        (
            "Invoking tool `get_weather` with input:\n"
            '```json\n{\n  "location": "LA"\n}\n```'
        ),
        "sunny and 72",
    ]
    assert channel.typing_active_at_send == [0, 0, 0]


@pytest.mark.asyncio
async def test_owner_dm_discards_transient_text_after_fresh_session_handoff() -> None:
    fake = FakeClient(
        stream_items=[
            {
                "type": "event",
                "event": {"type": "text_delta", "content": "old answer"},
            },
            {
                "type": "event",
                "event": {
                    "type": "tool_call",
                    "tool": "old-tool",
                    "input": {"value": "old"},
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "agentspace/session-restarted",
                    "restart_count": 1,
                },
            },
            {
                "type": "event",
                "event": {"type": "text_delta", "content": "fresh answer"},
            },
            {
                "type": "final",
                "completed": True,
                "automatic_restart_count": 1,
                "assistant_message": {"content": "fresh answer"},
            },
        ],
    )
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="new topic",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert channel.sent == ["fresh answer"]


@pytest.mark.asyncio
async def test_owner_dm_waits_for_complete_execute_tool_input() -> None:
    fake = FakeClient(
        stream_items=[
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-1",
                        "title": "provider-specific terminal",
                        "kind": "execute",
                        "rawInput": {"cwd": "/workspace"},
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "call-1",
                        "title": "printf 'hello\\n'",
                        "kind": "execute",
                        "rawInput": {
                            "command": "printf 'hello\\n'",
                            "cwd": "/workspace",
                        },
                    },
                },
            },
            {
                "type": "final",
                "completed": True,
                "assistant_message": {"content": ""},
            },
        ],
    )
    gateway = _ready_gateway(fake, DISCORD_CHUNK_MAX_CHARS="1900")
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="say hello",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert channel.sent == [
        (
            "Invoking tool `execute` with input:\n"
            "```json\n"
            "{\n"
            '  "command": "printf \'hello\\\\n\'",\n'
            '  "cwd": "/workspace"\n'
            "}\n"
            "```"
        ),
    ]


@pytest.mark.asyncio
async def test_owner_dm_merges_acp_other_tool_updates() -> None:
    fake = FakeClient(
        stream_items=[
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-skill",
                        "title": "skill",
                        "kind": "other",
                        "status": "pending",
                        "rawInput": {},
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "call-skill",
                        "status": "in_progress",
                        "rawInput": {"name": "firecrawl"},
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call",
                        "toolCallId": "call-refresh",
                        "title": "refresh index",
                        "kind": "other",
                        "status": "pending",
                        "rawInput": {},
                    },
                },
            },
            {
                "type": "event",
                "event": {
                    "type": "session/update",
                    "update": {
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "call-refresh",
                        "status": "completed",
                    },
                },
            },
            {
                "type": "final",
                "completed": True,
                "assistant_message": {"content": ""},
            },
        ],
    )
    gateway = _ready_gateway(fake, DISCORD_CHUNK_MAX_CHARS="1900")
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="search for news",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert channel.sent == [
        (
            "Invoking tool `skill` with input:\n"
            '```json\n{\n  "name": "firecrawl"\n}\n```'
        ),
        "Invoking tool `refresh index` with input:\n```json\n{}\n```",
    ]


@pytest.mark.asyncio
async def test_new_command_creates_session_and_sends_automated_reply() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="/new",
        channel=channel,
    )

    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert fake.sessions_created == [("agent-1", "discord:dm:111")]
    assert fake.sessions_reset == []
    assert fake.sent_messages == []
    assert channel.sent == [gw_mod.NEW_SESSION_STARTED_MESSAGE]
    assert gateway._session_id == "sess-1"  # type: ignore[reportPrivateUsage]  # noqa: SLF001


@pytest.mark.asyncio
async def test_new_command_resets_existing_session_and_preserves_session_id() -> None:
    fake = FakeClient(reply="ok")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    first_msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="first topic",
        channel=channel,
    )
    new_msg = FakeMessage(
        author=FakeAuthor(id=111),
        content=" /new ",
        channel=channel,
    )
    second_msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="second topic",
        channel=channel,
    )

    await gateway._on_message(cast("object", first_msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    await gateway._on_message(cast("object", new_msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    await gateway._on_message(cast("object", second_msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    assert fake.sessions_created == [("agent-1", "discord:dm:111")]
    assert fake.sessions_reset == ["sess-1"]
    assert fake.sent_messages == [("sess-1", "first topic"), ("sess-1", "second topic")]
    assert channel.sent == ["ok", gw_mod.NEW_SESSION_STARTED_MESSAGE, "ok"]


@pytest.mark.asyncio
async def test_non_owner_dm_is_ignored() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=999),
        content="hi",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert fake.sent_messages == []
    assert channel.sent == []


@pytest.mark.asyncio
async def test_guild_message_is_ignored() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
        guild=object(),
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert fake.sent_messages == []


@pytest.mark.asyncio
async def test_bot_author_is_ignored() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111, bot=True),
        content="hi",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert fake.sent_messages == []


@pytest.mark.asyncio
async def test_empty_content_is_ignored() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="   ",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert fake.sent_messages == []


@pytest.mark.asyncio
async def test_long_reply_is_chunked() -> None:
    fake = FakeClient(reply="aaaaaaaaaa\n\nbbbbbbbbbb\n\ncccccccccc")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="prompt",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert len(channel.sent) >= 2
    # Exactly one typing context wraps the agent call; chunked sends happen
    # afterwards with typing already closed.
    assert channel.typing_calls == 1
    assert channel.typing_active == 0
    # Every chunk was sent with typing already closed (no overlap that could
    # cause the indicator to briefly reappear after the reply).
    assert channel.typing_active_at_send == [0] * len(channel.sent)


@pytest.mark.asyncio
async def test_simulated_typing_sends_per_paragraph(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """With simulated typing on, each paragraph is its own message with delay.

    Validates the full pipeline:
      * agent reply is split into paragraphs
      * each paragraph is preceded by an asyncio.sleep
      * typing context is opened per paragraph
      * typing is closed BEFORE each send (no phantom indicator after the
        message lands)
    """
    reply = "first paragraph\n\nsecond paragraph\n\nthird paragraph"
    fake = FakeClient(reply=reply)
    gateway = _ready_gateway(
        fake,
        DISCORD_SIMULATED_TYPING_ENABLED="true",
        DISCORD_SIMULATED_TYPING_WPM="60",
    )

    sleep_calls: list[float] = []

    async def _record_sleep(delay: float) -> None:
        sleep_calls.append(delay)

    # Patch the symbol the gateway resolves at call time.
    monkeypatch.setattr(gw_mod.asyncio, "sleep", _record_sleep)

    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="prompt",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    # One Discord message per paragraph.
    assert channel.sent == [
        "first paragraph",
        "second paragraph",
        "third paragraph",
    ]
    # One typing context per paragraph (none for the agent call here because
    # the FakeClient returns instantly, but the agent-call typing context
    # still fires; that's fine).  Total = 1 (agent) + 3 (sim) = 4.
    assert channel.typing_calls == 4
    assert channel.typing_active == 0
    # Typing was closed before EACH send.
    assert channel.typing_active_at_send == [0, 0, 0]
    # Three simulated-typing sleeps were requested (one per paragraph), all
    # positive.
    assert len(sleep_calls) == 3
    assert all(d > 0 for d in sleep_calls)


@pytest.mark.asyncio
async def test_simulated_typing_disabled_uses_single_send() -> None:
    """With simulated typing off (default), behavior is the legacy path."""
    reply = "first paragraph\n\nsecond paragraph"
    fake = FakeClient(reply=reply)
    gateway = _ready_gateway(fake)
    # Default sim-typing config should be disabled.
    assert gateway._sim_typing_cfg == SimulatedTypingConfig(  # type: ignore[reportPrivateUsage]  # noqa: SLF001
        enabled=False,
        wpm=220,
    )

    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="prompt",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    # Single chunked send — _chunk preserves paragraph breaks but in this
    # case the whole reply fits in one Discord message (chunk_max=20 in
    # tests, but each paragraph fits, and _chunk packs paragraphs together
    # up to the limit).  Just assert: no per-paragraph sleeps requested.
    # (We can't easily assert this without monkeypatching sleep; instead
    # assert exactly one typing_call from the agent-call wrapper.)
    assert channel.typing_calls == 1


@pytest.mark.asyncio
async def test_send_message_failure_transitions_to_error() -> None:
    fake = FakeClient(fail_send=True)
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert gateway.status is GatewayStatus.ERROR
    assert gateway.last_error == "boom"
    assert any("agent error" in s for s in channel.sent)


@pytest.mark.asyncio
async def test_start_fails_without_token() -> None:
    fake = FakeClient()
    gateway = DiscordGateway()
    config = GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", fake),
        env={"DISCORD_OWNER_USER_ID": "111"},
    )
    await gateway.start(config)
    assert gateway.status is GatewayStatus.ERROR
    assert gateway.last_error is not None
    assert "DISCORD_BOT_TOKEN" in gateway.last_error


@pytest.mark.asyncio
async def test_start_fails_without_owner() -> None:
    fake = FakeClient()
    gateway = DiscordGateway()
    config = GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", fake),
        env={"DISCORD_BOT_TOKEN": "tok"},
    )
    await gateway.start(config)
    assert gateway.status is GatewayStatus.ERROR
    assert "DISCORD_OWNER_USER_ID" in (gateway.last_error or "")


@pytest.mark.asyncio
async def test_start_fails_with_non_numeric_owner() -> None:
    fake = FakeClient()
    gateway = DiscordGateway()
    config = GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", fake),
        env={"DISCORD_BOT_TOKEN": "tok", "DISCORD_OWNER_USER_ID": "abc"},
    )
    await gateway.start(config)
    assert gateway.status is GatewayStatus.ERROR
    assert "integer" in (gateway.last_error or "")


# --- Lifecycle test using a fake discord.Client factory ---------------------


class _FakeDiscordClient:
    def __init__(self) -> None:
        self.started_with: str | None = None
        self.closed = False
        self.user = type("U", (), {"name": "fake-bot"})()
        self._handlers: dict[str, object] = {}

    def event(self, fn: object) -> object:
        # discord.py's @client.event uses the function name as the event name.
        self._handlers[getattr(fn, "__name__", "")] = fn
        return fn

    async def start(self, token: str) -> None:
        self.started_with = token
        # Fire on_ready immediately
        on_ready = self._handlers.get("on_ready")
        if on_ready is not None:
            await cast("object", on_ready).__call__()  # type: ignore[attr-defined]
        # Run forever until cancelled
        await asyncio.Event().wait()

    async def close(self) -> None:
        self.closed = True


@pytest.mark.asyncio
async def test_lifecycle_start_then_stop() -> None:
    fake_client = _FakeDiscordClient()

    def factory(*, intents: object) -> object:  # noqa: ARG001
        return fake_client

    gateway = DiscordGateway(client_factory=cast("object", factory))  # type: ignore[arg-type]
    fake = FakeClient()
    config = _make_config(fake)

    await gateway.start(config)
    assert gateway.status is GatewayStatus.RUNNING
    assert fake_client.started_with == "fake-token"

    await gateway.stop()
    assert gateway.status is GatewayStatus.STOPPED
    assert fake_client.closed


# --- Router test ------------------------------------------------------------


@pytest.mark.asyncio
async def test_router_status_and_events() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001

    app = FastAPI()
    router = gateway.extra_router()
    app.include_router(router)
    client = TestClient(app)

    status = client.get("/gateway/status").json()
    assert status["status"] == "running"
    assert status["owner_id"] == 111
    assert status["session_id"] == "sess-1"

    events = client.get("/gateway/events").json()["events"]
    types = [event["type"] for event in events]
    assert "inbound" in types
    assert "outbound" in types


# --- Review-fix regression tests --------------------------------------------


@pytest.mark.asyncio
async def test_error_state_blocks_subsequent_messages() -> None:
    """Once the gateway is ERROR, further owner DMs are dropped (not retried)."""
    fake = FakeClient(fail_send=True)
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg1 = FakeMessage(
        author=FakeAuthor(id=111),
        content="first",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg1))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert gateway.status is GatewayStatus.ERROR
    sends_after_first = len(fake.sent_messages)

    msg2 = FakeMessage(
        author=FakeAuthor(id=111),
        content="second",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg2))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    # No additional client_service calls after entering ERROR.
    assert len(fake.sent_messages) == sends_after_first


@pytest.mark.asyncio
async def test_empty_assistant_reply_is_surfaced() -> None:
    fake = FakeClient(reply="")
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="hi",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert any("no reply" in s for s in channel.sent)
    events = list(gateway._events)  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    assert any(e.message and "empty assistant reply" in e.message for e in events)


@pytest.mark.asyncio
async def test_empty_owner_dm_logs_intent_hint() -> None:
    fake = FakeClient()
    gateway = _ready_gateway(fake)
    channel = FakeChannel()
    msg = FakeMessage(
        author=FakeAuthor(id=111),
        content="",
        channel=channel,
    )
    await gateway._on_message(cast("object", msg))  # type: ignore[arg-type, reportPrivateUsage]  # noqa: SLF001
    assert fake.sent_messages == []
    events = list(gateway._events)  # type: ignore[reportPrivateUsage]  # noqa: SLF001
    assert any(e.message and "Message Content" in e.message for e in events), (
        "expected an event hinting at the Message Content intent"
    )


@pytest.mark.asyncio
async def test_chunk_max_above_discord_limit_fails_start() -> None:
    fake = FakeClient()
    gateway = DiscordGateway()
    config = GatewayConfig(
        gateway_id="gw-1",
        agent_id="agent-1",
        client=cast("ClientServiceClient", fake),
        env={
            "DISCORD_BOT_TOKEN": "tok",
            "DISCORD_OWNER_USER_ID": "111",
            "DISCORD_CHUNK_MAX_CHARS": "5000",
        },
    )
    await gateway.start(config)
    assert gateway.status is GatewayStatus.ERROR
    assert "DISCORD_CHUNK_MAX_CHARS" in (gateway.last_error or "")


@pytest.mark.asyncio
async def test_crashed_client_task_flips_status_to_error() -> None:
    """If the discord client task ends with an exception, status flips to ERROR."""

    class _CrashingClient:
        def __init__(self) -> None:
            self.user = type("U", (), {"name": "fake-bot"})()
            self._handlers: dict[str, object] = {}
            self.closed = False

        def event(self, fn: object) -> object:
            self._handlers[getattr(fn, "__name__", "")] = fn
            return fn

        async def start(self, token: str) -> None:
            del token
            on_ready = self._handlers.get("on_ready")
            if on_ready is not None:
                await cast("object", on_ready).__call__()  # type: ignore[attr-defined]
            # Pretend the WSS died.
            msg = "wss closed"
            raise RuntimeError(msg)

        async def close(self) -> None:
            self.closed = True

    crashing = _CrashingClient()

    def factory(*, intents: object) -> object:  # noqa: ARG001
        return crashing

    gateway = DiscordGateway(client_factory=cast("object", factory))  # type: ignore[arg-type]
    fake = FakeClient()
    config = _make_config(fake)
    await gateway.start(config)

    # Give the done-callback a chance to run on the next event-loop tick.
    await asyncio.sleep(0)
    await asyncio.sleep(0)

    assert gateway.status is GatewayStatus.ERROR
    assert gateway.last_error is not None
    assert "crashed" in gateway.last_error
