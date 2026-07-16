"""Discord gateway implementation.

Bridges a single 1:1 Discord direct message conversation to an AgentSpace
agent through ``client_service``.

The implementation is intentionally narrow.  Anything resembling guild
support, streaming edits, or per-channel session routing is explicitly out of
scope for v1.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
from collections import deque
from dataclasses import dataclass
from typing import TYPE_CHECKING, Protocol, cast

import discord
from fastapi import APIRouter
from gateway.commands import CommandInvocation, GatewayCommand, GatewayCommandRegistry
from gateway.events import GatewayEvent, GatewayEventType
from gateway.protocol import GatewayConfig, GatewayStatus, GatewayType
from gateway.simulated_typing import (
    DEFAULT_WPM,
    SimulatedTypingConfig,
    plan_simulated_typing,
)

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)

EVENTS_LIMIT = 200
DEFAULT_CHUNK_MAX_CHARS = 1900
DISCORD_MAX_MESSAGE_CHARS = 2000
LOGIN_TIMEOUT_S = 30.0
NEW_SESSION_COMMAND_NAME = "new"
NEW_SESSION_STARTED_MESSAGE = "a new session has started"


class _ClientFactory(Protocol):
    def __call__(self, *, intents: discord.Intents) -> discord.Client: ...


def _default_client_factory(*, intents: discord.Intents) -> discord.Client:
    return discord.Client(intents=intents)


def _parse_bool_env(value: str) -> bool:
    """Parse a permissive boolean from an env-string."""
    return value.strip().lower() in {"1", "true", "yes", "on"}


def _chunk(text: str, max_chars: int) -> list[str]:  # noqa: C901
    r"""Split ``text`` into chunks no larger than ``max_chars`` characters.

    Splits on paragraph boundaries first (``\n\n``), then line boundaries
    (``\n``), and finally hard-cuts within a line if necessary.  Empty
    chunks are never produced.  Pure function — no Discord dependency.
    """
    if max_chars <= 0:
        msg = "max_chars must be positive"
        raise ValueError(msg)
    stripped = text.strip()
    if not stripped:
        return []
    if len(stripped) <= max_chars:
        return [stripped]

    chunks: list[str] = []
    current = ""

    def flush() -> None:
        nonlocal current
        if current:
            chunks.append(current)
            current = ""

    def append_atom(atom: str, sep: str) -> None:
        nonlocal current
        if not current:
            current = atom
            return
        candidate = current + sep + atom
        if len(candidate) <= max_chars:
            current = candidate
        else:
            flush()
            current = atom

    for paragraph in stripped.split("\n\n"):
        if not paragraph:
            continue
        if len(paragraph) <= max_chars:
            append_atom(paragraph, "\n\n")
            continue
        # paragraph too long — split on lines
        flush()
        for line in paragraph.split("\n"):
            if len(line) <= max_chars:
                append_atom(line, "\n")
                continue
            # line too long — hard split
            flush()
            chunks.extend(
                line[start : start + max_chars]
                for start in range(0, len(line), max_chars)
            )
    flush()
    return chunks


class _ChannelLike(Protocol):
    async def send(self, content: str) -> object: ...

    def typing(self) -> contextlib.AbstractAsyncContextManager[object]: ...


class _MessageLike(Protocol):
    async def add_reaction(self, emoji: str) -> object: ...

    async def remove_reaction(self, emoji: str, member: object) -> object: ...


@dataclass(frozen=True, slots=True)
class _DiscordCommandContext:
    channel: _ChannelLike
    sender: str


# Reactions used to give the user visible feedback about turn progress on
# their own message:
#   * EYES is added once the gateway acquires its send lock and starts
#     processing the message (the agent may take a while to reply).
#   * CHECK MARK replaces EYES once the assistant reply has been delivered.
_PROCESSING_REACTION = "\N{EYES}"
_DONE_REACTION = "\N{WHITE HEAVY CHECK MARK}"


@dataclass(frozen=True, slots=True)
class _ToolInvocation:
    key: str | None
    tool: str
    input: object


class DiscordGateway:
    """Single-DM Discord gateway."""

    def __init__(
        self,
        *,
        client_factory: _ClientFactory | None = None,
    ) -> None:
        self._status = GatewayStatus.STOPPED
        self._last_error: str | None = None
        self._config: GatewayConfig | None = None
        self._client: discord.Client | None = None
        self._client_task: asyncio.Task[None] | None = None
        self._session_id: str | None = None
        self._owner_id: int | None = None
        self._chunk_max: int = DEFAULT_CHUNK_MAX_CHARS
        self._sim_typing_cfg: SimulatedTypingConfig = SimulatedTypingConfig(
            enabled=False,
        )
        self._send_lock = asyncio.Lock()
        self._events: deque[GatewayEvent] = deque(maxlen=EVENTS_LIMIT)
        self._ready_event = asyncio.Event()
        self._client_factory: _ClientFactory = client_factory or _default_client_factory
        self._commands = GatewayCommandRegistry[_DiscordCommandContext](
            [
                GatewayCommand(
                    name=NEW_SESSION_COMMAND_NAME,
                    description="Start a fresh agent session.",
                    handler=self._handle_new_session_command,
                ),
            ],
        )

    @property
    def name(self) -> str:
        return GatewayType.DISCORD.value

    @property
    def status(self) -> GatewayStatus:
        return self._status

    @property
    def last_error(self) -> str | None:
        return self._last_error

    async def start(self, config: GatewayConfig) -> None:  # noqa: PLR0911
        self._config = config
        self._status = GatewayStatus.STARTING
        self._last_error = None
        self._session_id = None
        self._ready_event = asyncio.Event()

        token = config.env.get("DISCORD_BOT_TOKEN")
        if not token:
            self._fail("DISCORD_BOT_TOKEN is required")
            return

        owner_raw = config.env.get("DISCORD_OWNER_USER_ID")
        if not owner_raw:
            self._fail("DISCORD_OWNER_USER_ID is required")
            return
        try:
            self._owner_id = int(owner_raw)
        except ValueError:
            self._fail(f"DISCORD_OWNER_USER_ID must be an integer, got {owner_raw!r}")
            return

        try:
            self._chunk_max = int(
                config.env.get("DISCORD_CHUNK_MAX_CHARS", DEFAULT_CHUNK_MAX_CHARS),
            )
        except ValueError as exc:
            self._fail(f"invalid numeric env: {exc}")
            return

        if self._chunk_max <= 0 or self._chunk_max >= DISCORD_MAX_MESSAGE_CHARS:
            limit = DISCORD_MAX_MESSAGE_CHARS - 1
            self._fail(f"DISCORD_CHUNK_MAX_CHARS must be in 1..{limit}")
            return

        sim_enabled = _parse_bool_env(
            config.env.get("DISCORD_SIMULATED_TYPING_ENABLED", "false"),
        )
        try:
            sim_wpm = int(
                config.env.get("DISCORD_SIMULATED_TYPING_WPM", DEFAULT_WPM),
            )
        except ValueError as exc:
            self._fail(f"DISCORD_SIMULATED_TYPING_WPM must be an integer: {exc}")
            return
        if sim_wpm <= 0:
            self._fail("DISCORD_SIMULATED_TYPING_WPM must be > 0")
            return
        self._sim_typing_cfg = SimulatedTypingConfig(
            enabled=sim_enabled,
            wpm=sim_wpm,
        )

        intents = discord.Intents.default()
        intents.message_content = True
        client = self._client_factory(intents=intents)
        self._client = client
        self._wire_handlers(client)

        logger.info(
            "discord gateway starting: gateway_id=%s agent_id=%s owner_id=%s "
            "(token length=%d)",
            config.gateway_id,
            config.agent_id,
            self._owner_id,
            len(token),
        )

        self._client_task = asyncio.create_task(client.start(token))
        self._client_task.add_done_callback(self._on_client_task_done)

        try:
            await asyncio.wait_for(self._ready_event.wait(), timeout=LOGIN_TIMEOUT_S)
        except TimeoutError:
            self._fail("discord login timed out")
            await self._cleanup_client()
            return
        except Exception as exc:  # noqa: BLE001 - want to surface every login failure
            self._fail(f"discord login failed: {exc}")
            await self._cleanup_client()
            return

        self._status = GatewayStatus.RUNNING
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.STATUS,
                message="discord gateway started",
            ),
        )

    async def stop(self) -> None:
        await self._cleanup_client()
        self._status = GatewayStatus.STOPPED
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.STATUS,
                message="discord gateway stopped",
            ),
        )
        logger.info("discord gateway stopped")

    def extra_router(self) -> APIRouter:
        router = APIRouter(prefix="/gateway", tags=["discord-gateway"])

        @router.get("/status")
        async def status() -> dict[str, object]:
            return {
                "status": self._status.value,
                "owner_id": self._owner_id,
                "session_id": self._session_id,
                "last_error": self._last_error,
            }

        @router.get("/events")
        async def events() -> dict[str, object]:
            return {"events": [event.to_dict() for event in self._events]}

        _ = (status, events)
        return router

    # ----- internal helpers -----

    def _wire_handlers(self, client: discord.Client) -> None:
        @client.event  # type: ignore[misc]
        async def on_ready() -> None:
            user = client.user
            logger.info(
                "discord gateway logged in as %s",
                getattr(user, "name", "?"),
            )
            self._ready_event.set()

        @client.event  # type: ignore[misc]
        async def on_message(message: discord.Message) -> None:
            await self._on_message(message)

        _ = (on_ready, on_message)

    async def _on_message(self, message: discord.Message) -> None:  # noqa: C901, PLR0911
        if self._config is None:
            return
        # ERROR is terminal until the gateway is restarted.
        if self._status is not GatewayStatus.RUNNING:
            return
        author = message.author
        if author.bot:
            return
        if message.guild is not None:
            return
        if self._owner_id is None or author.id != self._owner_id:
            self._record_event(
                GatewayEvent(
                    type=GatewayEventType.STATUS,
                    sender=str(author.id),
                    message="ignored non-owner DM",
                ),
            )
            return
        text = (message.content or "").strip()
        if not text:
            # Most likely cause: the privileged "Message Content" intent is
            # disabled in the Discord Developer Portal, so discord.py delivers
            # the message with empty content.  Surface a hint instead of
            # silently dropping the turn.
            self._record_event(
                GatewayEvent(
                    type=GatewayEventType.STATUS,
                    sender=str(author.id),
                    message=(
                        "received owner DM with empty content — "
                        "check that the 'Message Content' intent is enabled "
                        "for this bot in the Discord Developer Portal"
                    ),
                ),
            )
            return

        self._record_event(
            GatewayEvent(
                type=GatewayEventType.INBOUND,
                sender=str(author.id),
                content=text,
            ),
        )

        async with self._send_lock:
            channel = cast("_ChannelLike", message.channel)
            if await self._dispatch_command(
                text,
                _DiscordCommandContext(channel=channel, sender=str(author.id)),
            ):
                return

            # Drop the EYES reaction up front so the user sees we've picked
            # the message up before the (potentially slow) agent call begins.
            await self._react(message, _PROCESSING_REACTION)

            try:
                session_id = await self._ensure_session()
            except Exception as exc:  # noqa: BLE001 - any failure should ERROR the gateway
                await self._handle_send_failure(
                    message.channel,
                    sender=str(author.id),
                    session_id=None,
                    exc=exc,
                )
                return

            try:
                delivered = await self._deliver_streamed_response(
                    channel,
                    session_id=session_id,
                    message=text,
                    sender=str(author.id),
                )
            except Exception as exc:  # noqa: BLE001 - any failure should ERROR the gateway
                await self._handle_send_failure(
                    message.channel,
                    sender=str(author.id),
                    session_id=session_id,
                    exc=exc,
                )
                return

            if not delivered:
                return

            # Turn delivered: swap the in-flight EYES for a CHECK MARK so
            # the user can see at a glance which past messages have been
            # answered (and which are still pending behind the lock).
            await self._swap_reaction(
                message,
                from_emoji=_PROCESSING_REACTION,
                to_emoji=_DONE_REACTION,
            )

    @contextlib.asynccontextmanager
    async def _typing_indicator(
        self,
        channel: _ChannelLike,
    ) -> AsyncIterator[None]:
        """Wrap channel.typing() so its own failures cannot drop the turn.

        discord.py's channel.typing() opens a REST call on enter; that call
        can fail (transient 5xx, missing Send Messages permission, deleted
        channel, etc.).  Typing is purely cosmetic, so on entry failure we
        log at debug and execute the body without the indicator.

        Body exceptions are NOT swallowed \u2014 the caller's own try/except
        blocks must continue to see them.  Exit failures from typing are
        swallowed because they only affect the indicator, not the work.
        """
        try:
            cm = channel.typing()
            await cm.__aenter__()
        except Exception as exc:  # noqa: BLE001 - cosmetic, never drop a turn
            logger.debug("discord typing() enter failed: %s", exc)
            yield
            return

        try:
            yield
        finally:
            try:
                await cm.__aexit__(None, None, None)
            except Exception as exc:  # noqa: BLE001 - cosmetic, never drop a turn
                logger.debug("discord typing() exit failed: %s", exc)

    async def _react(self, message: object, emoji: str) -> None:
        """Add a reaction; log and swallow failures (cosmetic UX only)."""
        try:
            await cast("_MessageLike", message).add_reaction(emoji)
        except Exception as exc:  # noqa: BLE001 - cosmetic, must not abort the turn
            logger.debug("discord add_reaction(%r) failed: %s", emoji, exc)

    async def _swap_reaction(
        self,
        message: object,
        *,
        from_emoji: str,
        to_emoji: str,
    ) -> None:
        """Replace one of our own reactions with another. Failures are logged."""
        client = self._client
        if client is None:
            return
        try:
            await cast("_MessageLike", message).remove_reaction(
                from_emoji,
                cast("object", client.user),
            )
        except Exception as exc:  # noqa: BLE001 - cosmetic, must not abort the turn
            logger.debug("discord remove_reaction(%r) failed: %s", from_emoji, exc)
        await self._react(message, to_emoji)

    async def _ensure_session(self) -> str:
        assert self._config is not None  # noqa: S101 - invariant
        if self._session_id is not None:
            return self._session_id
        session = await self._config.client.create_session(
            agent_id=self._config.agent_id,
            channel_name=f"discord:dm:{self._owner_id}",
        )
        self._session_id = str(session["session_id"])
        return self._session_id

    async def _handle_new_session_command(
        self,
        invocation: CommandInvocation,
        context: _DiscordCommandContext,
    ) -> None:
        del invocation
        try:
            session_id = await self._start_new_session()
        except Exception as exc:  # noqa: BLE001 - any failure should ERROR the gateway
            await self._handle_send_failure(
                context.channel,
                sender=context.sender,
                session_id=self._session_id,
                exc=exc,
            )
            return

        try:
            await context.channel.send(NEW_SESSION_STARTED_MESSAGE)
        except Exception as exc:  # noqa: BLE001 - Discord send failure is operational
            await self._handle_send_failure(
                context.channel,
                sender=context.sender,
                session_id=session_id,
                exc=exc,
            )
            return

        self._record_event(
            GatewayEvent(
                type=GatewayEventType.OUTBOUND,
                sender=context.sender,
                content=NEW_SESSION_STARTED_MESSAGE,
                session_id=session_id,
            ),
        )

    async def _dispatch_command(
        self,
        text: str,
        context: _DiscordCommandContext,
    ) -> bool:
        try:
            result = await self._commands.dispatch(text, context)
        except Exception as exc:  # noqa: BLE001 - command failures are operational
            await self._handle_send_failure(
                context.channel,
                sender=context.sender,
                session_id=self._session_id,
                exc=exc,
            )
            return True
        return result.handled

    async def _start_new_session(self) -> str:
        assert self._config is not None  # noqa: S101 - invariant
        if self._session_id is None:
            return await self._ensure_session()

        session = await self._config.client.reset_session(session_id=self._session_id)
        self._session_id = str(session["session_id"])
        return self._session_id

    async def _deliver_streamed_response(  # noqa: C901, PLR0912, PLR0915
        self,
        channel: _ChannelLike,
        *,
        session_id: str,
        message: str,
        sender: str,
    ) -> bool:
        assert self._config is not None  # noqa: S101 - invariant
        pending_text: list[str] = []
        sent_assistant_text = False
        sent_any_message = False
        sent_tool_call_count = 0
        sent_tool_call_keys: set[str] = set()
        acp_tool_calls: dict[str, dict[str, object]] = {}
        final_reply = ""
        final_item: dict[str, object] | None = None
        final_received = False
        typing_cm = self._typing_indicator(channel)
        typing_open = False

        async def close_typing() -> None:
            nonlocal typing_open
            if not typing_open:
                return
            await typing_cm.__aexit__(None, None, None)
            typing_open = False

        async def send_outbound(
            content: str,
            *,
            assistant_text: bool,
            use_simulated_typing: bool = False,
        ) -> bool:
            nonlocal sent_any_message, sent_assistant_text
            if not content.strip():
                return True
            await close_typing()
            try:
                if use_simulated_typing:
                    await self._deliver_reply(channel, content)
                else:
                    await self._send_chunked(channel, content)
            except Exception as exc:  # noqa: BLE001 - keep gateway alive
                logger.warning("discord send failed: %s", exc)
                self._record_event(
                    GatewayEvent(
                        type=GatewayEventType.ERROR,
                        sender=sender,
                        message=f"discord send failed: {exc}",
                        session_id=session_id,
                    ),
                )
                return False
            self._record_event(
                GatewayEvent(
                    type=GatewayEventType.OUTBOUND,
                    sender=sender,
                    content=content,
                    session_id=session_id,
                ),
            )
            sent_any_message = True
            if assistant_text:
                sent_assistant_text = True
            return True

        async def flush_pending_text() -> bool:
            if not pending_text:
                return True
            content = "".join(pending_text)
            pending_text.clear()
            return await send_outbound(content, assistant_text=True)

        async def send_tool_invocation(invocation: _ToolInvocation) -> bool:
            nonlocal sent_tool_call_count
            if invocation.key is not None:
                if invocation.key in sent_tool_call_keys:
                    return True
                sent_tool_call_keys.add(invocation.key)
            sent_tool_call_count += 1
            return await send_outbound(
                _format_tool_call_message(invocation.tool, invocation.input),
                assistant_text=False,
            )

        await typing_cm.__aenter__()
        typing_open = True
        try:
            async for item in self._config.client.stream_message(
                session_id=session_id,
                message=message,
            ):
                item_type = item.get("type")
                if item_type == "event":
                    raw_event = item.get("event")
                    if not isinstance(raw_event, dict):
                        continue
                    event = cast("dict[str, object]", raw_event)
                    text_delta = _stream_event_text(event)
                    if text_delta:
                        pending_text.append(text_delta)
                    tool_invocation = _tool_invocation_from_event(
                        event,
                        acp_tool_calls,
                    )
                    if tool_invocation is not None:
                        if not await flush_pending_text():
                            return False
                        if not await send_tool_invocation(tool_invocation):
                            return False
                    continue

                if item_type == "final":
                    final_received = True
                    final_item = item
                    if item.get("completed") is False:
                        error = item.get("error")
                        msg = error if isinstance(error, str) else "agent stream failed"
                        raise RuntimeError(msg)
                    final_reply = _extract_assistant_text(item)
                    break

            if not final_received:
                msg = "client_service stream ended without final payload"
                raise RuntimeError(msg)

            if not await flush_pending_text():
                return False

            if sent_tool_call_count == 0:
                assert final_item is not None  # noqa: S101 - final_received invariant
                for tool_call_message in _extract_tool_call_messages(final_item):
                    if not await send_outbound(
                        tool_call_message,
                        assistant_text=False,
                    ):
                        return False

            if (
                not sent_assistant_text
                and final_reply
                and not await send_outbound(
                    final_reply,
                    assistant_text=True,
                    use_simulated_typing=True,
                )
            ):
                return False

            if not sent_any_message:
                # Agent produced no assistant text — don't ghost the user.
                self._record_event(
                    GatewayEvent(
                        type=GatewayEventType.STATUS,
                        sender=sender,
                        message="agent returned empty assistant reply",
                        session_id=session_id,
                    ),
                )
                await close_typing()
                with contextlib.suppress(Exception):
                    await channel.send("(agent produced no reply)")

            return True
        finally:
            await close_typing()

    async def _deliver_reply(self, channel: _ChannelLike, reply: str) -> None:
        """Deliver an assistant reply, optionally with simulated typing.

        With simulated typing disabled, behaves like the previous
        single-message path: chunked-send the whole reply.

        With it enabled, splits the reply into paragraph-sized pieces and
        sleeps under a typing indicator before each one, sized by WPM.
        Each typing context is closed *before* the corresponding send so
        Discord's typing indicator never lingers visibly after a message
        lands (see commit 77cb0d4).
        """
        chunks = plan_simulated_typing(reply, self._sim_typing_cfg)
        for piece in chunks:
            if piece.delay_s > 0:
                async with self._typing_indicator(channel):
                    await asyncio.sleep(piece.delay_s)
            await self._send_chunked(channel, piece.content)

    async def _send_chunked(self, channel: _ChannelLike, text: str) -> None:
        # Final safety pass: if any single planned chunk still exceeds
        # Discord's 2000-char limit (e.g. a giant code block in one
        # paragraph), split it down to size before sending.
        for chunk in _chunk(text, self._chunk_max):
            await channel.send(chunk)

    async def _handle_send_failure(
        self,
        channel: object,
        *,
        sender: str,
        session_id: str | None,
        exc: BaseException,
    ) -> None:
        self._last_error = str(exc)
        self._status = GatewayStatus.ERROR
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.ERROR,
                sender=sender,
                message=str(exc),
                session_id=session_id,
            ),
        )
        logger.exception(
            "discord gateway send_message failed; transitioning to ERROR "
            "(restart required to recover)",
            exc_info=exc,
        )
        with contextlib.suppress(Exception):
            await cast("_ChannelLike", channel).send(
                "⚠️ agent error — check gateway logs",
            )

    async def _cleanup_client(self) -> None:
        client = self._client
        if client is not None:
            with contextlib.suppress(Exception):
                await client.close()
        task = self._client_task
        if task is not None and not task.done():
            task.cancel()
            with contextlib.suppress(BaseException):
                await task
        self._client = None
        self._client_task = None

    def _on_client_task_done(self, task: asyncio.Task[None]) -> None:
        if task.cancelled():
            return
        exc = task.exception()
        if exc is None:
            return
        # The discord client task ended unexpectedly while we still believed
        # the gateway was healthy.  Flip to ERROR so /gateway/status reflects
        # reality without waiting for the next inbound DM to fail.
        if self._status in (GatewayStatus.RUNNING, GatewayStatus.STARTING):
            self._status = GatewayStatus.ERROR
            self._last_error = f"discord client task crashed: {exc}"
            self._record_event(
                GatewayEvent(
                    type=GatewayEventType.ERROR,
                    message=self._last_error,
                ),
            )
            logger.error(
                "discord client task ended unexpectedly: %s",
                exc,
            )

    def _fail(self, reason: str) -> None:
        self._status = GatewayStatus.ERROR
        self._last_error = reason
        self._record_event(
            GatewayEvent(
                type=GatewayEventType.ERROR,
                message=reason,
            ),
        )
        logger.error("discord gateway failed to start: %s", reason)

    def _record_event(self, event: GatewayEvent) -> None:
        self._events.append(event)


def _extract_assistant_text(response: dict[str, object]) -> str:
    assistant = response.get("assistant_message")
    if isinstance(assistant, dict):
        content = cast("dict[str, object]", assistant).get("content")
        if isinstance(content, str):
            return content
    return ""


def _extract_tool_call_messages(response: dict[str, object]) -> list[str]:
    assistant = response.get("assistant_message")
    if not isinstance(assistant, dict):
        return []
    tool_calls = cast("dict[str, object]", assistant).get("tool_calls")
    if not isinstance(tool_calls, list):
        return []

    messages: list[str] = []
    for raw_tool_call in cast("list[object]", tool_calls):
        if not isinstance(raw_tool_call, dict):
            continue
        tool_call = cast("dict[str, object]", raw_tool_call)
        tool = tool_call.get("tool")
        if not isinstance(tool, str) or not tool:
            continue
        messages.append(_format_tool_call_message(tool, tool_call.get("input")))
    return messages


def _stream_event_text(event: dict[str, object]) -> str:
    event_type = event.get("type")
    if event_type == "text_delta":
        content = event.get("content")
        return content if isinstance(content, str) else ""
    if event_type != "session/update":
        return ""
    update = event.get("update")
    if not isinstance(update, dict):
        return ""
    update_dict = cast("dict[str, object]", update)
    if update_dict.get("sessionUpdate") != "agent_message_chunk":
        return ""
    return _content_text(update_dict.get("content"))


def _tool_invocation_from_event(
    event: dict[str, object],
    acp_tool_calls: dict[str, dict[str, object]],
) -> _ToolInvocation | None:
    event_type = event.get("type")
    if event_type == "tool_call":
        return _legacy_tool_invocation(event)
    if event_type == "session/update":
        return _acp_tool_invocation(event, acp_tool_calls)
    return None


def _legacy_tool_invocation(event: dict[str, object]) -> _ToolInvocation | None:
    tool = event.get("tool")
    if not isinstance(tool, str) or not tool:
        return None
    return _ToolInvocation(key=None, tool=tool, input=event.get("input"))


def _acp_tool_invocation(
    event: dict[str, object],
    tool_calls: dict[str, dict[str, object]],
) -> _ToolInvocation | None:
    update = event.get("update")
    if not isinstance(update, dict):
        return None
    update_dict = cast("dict[str, object]", update)
    update_type = update_dict.get("sessionUpdate")
    tool_call_id = _optional_non_empty_string(update_dict.get("toolCallId"))
    if update_type == "tool_call":
        if tool_call_id is None:
            return _acp_tool_invocation_from_state(update_dict, None)
        state = dict(update_dict)
        tool_calls[tool_call_id] = state
    elif update_type == "tool_call_update" and tool_call_id is not None:
        # ACP updates are patches, so omitted fields retain their prior values.
        state = tool_calls.setdefault(tool_call_id, {})
        state.update(update_dict)
    else:
        return None

    terminal = state.get("status") in {"completed", "failed"}
    if not terminal and not _has_displayable_acp_tool_input(state):
        return None
    invocation = _acp_tool_invocation_from_state(state, tool_call_id)
    if terminal:
        del tool_calls[tool_call_id]
    return invocation


def _has_displayable_acp_tool_input(state: dict[str, object]) -> bool:
    if "rawInput" not in state:
        return False
    raw_input = state.get("rawInput")
    if raw_input is None:
        return False
    if not isinstance(raw_input, dict):
        return True
    input_dict = cast("dict[str, object]", raw_input)
    if not input_dict:
        return False
    return state.get("kind") != "execute" or not set(input_dict).issubset(
        {"cwd", "workdir"},
    )


def _acp_tool_invocation_from_state(
    state: dict[str, object],
    tool_call_id: str | None,
) -> _ToolInvocation:
    tool = _acp_tool_name(state, tool_call_id)
    tool_input = state.get("rawInput") if "rawInput" in state else None
    return _ToolInvocation(key=tool_call_id, tool=tool, input=tool_input)


def _acp_tool_name(update: dict[str, object], tool_call_id: str | None) -> str:
    kind = _optional_non_empty_string(update.get("kind"))
    title = _optional_non_empty_string(update.get("title"))
    if kind == "other":
        return title or kind
    return kind or title or tool_call_id or "tool"


def _optional_non_empty_string(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _content_text(content: object) -> str:
    if isinstance(content, list):
        return "".join(_content_text(item) for item in cast("list[object]", content))
    if content is None:
        return ""
    if not isinstance(content, dict):
        return str(content)
    content_dict = cast("dict[str, object]", content)
    content_type = content_dict.get("type")
    if content_type == "text":
        text = content_dict.get("text")
        return text if isinstance(text, str) else ""
    if content_type == "content":
        return _content_text(content_dict.get("content"))
    return json.dumps(content_dict, separators=(",", ":"))


def _format_tool_call_message(tool: str, tool_input: object) -> str:
    tool_name = tool.replace("`", "\\`")
    input_text, language = _tool_input_text(tool_input)
    if not input_text:
        return f"Invoking tool `{tool_name}` with no input."
    return f"Invoking tool `{tool_name}` with input:\n```{language}\n{input_text}\n```"


def _tool_input_text(tool_input: object) -> tuple[str, str]:
    if tool_input is None:
        return "", ""
    if isinstance(tool_input, str):
        text = tool_input.strip()
        language = "json" if text.startswith(("{", "[")) else ""
        return text, language
    return json.dumps(tool_input, indent=2), "json"
