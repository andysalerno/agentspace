"""Discord gateway implementation.

Bridges a single 1:1 Discord direct message conversation to an AgentSpace
agent through ``client_service``.

The implementation is intentionally narrow — see ``DISCORD_PLAN.md`` for the
locked-down scope.  Anything resembling guild support, slash commands,
streaming edits, or per-channel session routing is explicitly out of scope
for v1.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
from collections import deque
from typing import TYPE_CHECKING, Protocol, cast

import discord
from fastapi import APIRouter
from gateway.events import GatewayEvent, GatewayEventType
from gateway.protocol import GatewayConfig, GatewayStatus, GatewayType

if TYPE_CHECKING:
    from collections.abc import AsyncIterator

logger = logging.getLogger(__name__)

EVENTS_LIMIT = 200
DEFAULT_CHUNK_MAX_CHARS = 1900
DISCORD_MAX_MESSAGE_CHARS = 2000
LOGIN_TIMEOUT_S = 30.0


class _ClientFactory(Protocol):
    def __call__(self, *, intents: discord.Intents) -> discord.Client: ...


def _default_client_factory(*, intents: discord.Intents) -> discord.Client:
    return discord.Client(intents=intents)


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


# Reactions used to give the user visible feedback about turn progress on
# their own message:
#   * EYES is added once the gateway acquires its send lock and starts
#     processing the message (the agent may take a while to reply).
#   * CHECK MARK replaces EYES once the assistant reply has been delivered.
_PROCESSING_REACTION = "\N{EYES}"
_DONE_REACTION = "\N{WHITE HEAVY CHECK MARK}"


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
        self._send_lock = asyncio.Lock()
        self._events: deque[GatewayEvent] = deque(maxlen=EVENTS_LIMIT)
        self._ready_event = asyncio.Event()
        self._client_factory: _ClientFactory = client_factory or _default_client_factory

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
        # ERROR is terminal until the gateway is restarted (see DISCORD_PLAN.md).
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

            # Wrap *only* the agent call in the typing indicator.  This is
            # the part of the turn the user is genuinely waiting on; the
            # reaction swap and message send afterwards are sub-second
            # operations and don't deserve a typing indicator.
            #
            # Keeping typing() narrow also avoids a real artefact: discord.py
            # auto-refreshes typing on a timer, and Discord clears typing on
            # the next message we send.  If we held typing() open across
            # _send_chunked, the background refresh task could re-assert
            # typing right after the reply landed, so the user would see the
            # indicator briefly reappear after the response.
            try:
                async with self._typing_indicator(channel):
                    response = await self._config.client.send_message(
                        session_id=session_id,
                        message=text,
                    )
            except Exception as exc:  # noqa: BLE001 - any failure should ERROR the gateway
                await self._handle_send_failure(
                    message.channel,
                    sender=str(author.id),
                    session_id=session_id,
                    exc=exc,
                )
                return

            reply = _extract_assistant_text(response)
            if not reply:
                # Agent produced no assistant text — don't ghost the user.
                self._record_event(
                    GatewayEvent(
                        type=GatewayEventType.STATUS,
                        sender=str(author.id),
                        message="agent returned empty assistant reply",
                        session_id=session_id,
                    ),
                )
                with contextlib.suppress(Exception):
                    await channel.send("(agent produced no reply)")
                return

            try:
                await self._send_chunked(channel, reply)
            except Exception as exc:  # noqa: BLE001 - keep gateway alive
                logger.warning("discord send failed: %s", exc)
                self._record_event(
                    GatewayEvent(
                        type=GatewayEventType.ERROR,
                        sender=str(author.id),
                        message=f"discord send failed: {exc}",
                        session_id=session_id,
                    ),
                )
                return

            # Reply delivered: swap the in-flight EYES for a CHECK MARK so
            # the user can see at a glance which past messages have been
            # answered (and which are still pending behind the lock).
            await self._swap_reaction(
                message,
                from_emoji=_PROCESSING_REACTION,
                to_emoji=_DONE_REACTION,
            )

            self._record_event(
                GatewayEvent(
                    type=GatewayEventType.OUTBOUND,
                    sender=str(author.id),
                    content=reply,
                    session_id=session_id,
                ),
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

    async def _send_chunked(self, channel: _ChannelLike, text: str) -> None:
        # The outer turn already holds an open channel.typing() context, so
        # the indicator stays on between chunks for free; no inter-chunk
        # sleep is needed.
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
