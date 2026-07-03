"""Shared slash-command parsing and dispatch helpers for gateways."""

from __future__ import annotations

import re
from collections.abc import Awaitable, Callable, Iterable
from dataclasses import dataclass

type GatewayCommandHandler[ContextT] = Callable[
    ["CommandInvocation", ContextT],
    Awaitable[None],
]

_COMMAND_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]*$")


@dataclass(frozen=True, slots=True)
class CommandInvocation:
    raw: str
    name: str
    args: str
    argv: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class GatewayCommand[ContextT]:
    name: str
    description: str
    handler: GatewayCommandHandler[ContextT]
    aliases: tuple[str, ...] = ()


@dataclass(frozen=True, slots=True)
class CommandDispatchResult[ContextT]:
    handled: bool
    invocation: CommandInvocation | None = None
    command: GatewayCommand[ContextT] | None = None


class GatewayCommandRegistry[ContextT]:
    """Case-insensitive registry for gateway slash commands."""

    def __init__(
        self,
        commands: Iterable[GatewayCommand[ContextT]] = (),
        *,
        prefix: str = "/",
    ) -> None:
        if not prefix:
            msg = "command prefix must not be empty"
            raise ValueError(msg)
        self._prefix = prefix
        self._commands: dict[str, GatewayCommand[ContextT]] = {}
        for command in commands:
            self.register(command)

    @property
    def prefix(self) -> str:
        return self._prefix

    def register(self, command: GatewayCommand[ContextT]) -> None:
        for name in (command.name, *command.aliases):
            normalized = _normalize_command_name(name)
            if normalized is None:
                msg = f"invalid command name: {name!r}"
                raise ValueError(msg)
            if normalized in self._commands:
                msg = f"duplicate command name or alias: {name!r}"
                raise ValueError(msg)
            self._commands[normalized] = command

    def parse(self, text: str) -> CommandInvocation | None:
        return parse_command_text(text, prefix=self._prefix)

    def command_for(
        self,
        invocation: CommandInvocation,
    ) -> GatewayCommand[ContextT] | None:
        return self._commands.get(invocation.name)

    async def dispatch(
        self,
        text: str,
        context: ContextT,
    ) -> CommandDispatchResult[ContextT]:
        invocation = self.parse(text)
        if invocation is None:
            return CommandDispatchResult(handled=False)
        command = self.command_for(invocation)
        if command is None:
            return CommandDispatchResult(handled=False, invocation=invocation)
        await command.handler(invocation, context)
        return CommandDispatchResult(
            handled=True,
            invocation=invocation,
            command=command,
        )


def parse_command_text(text: str, *, prefix: str = "/") -> CommandInvocation | None:
    if not prefix:
        msg = "command prefix must not be empty"
        raise ValueError(msg)

    raw = text.strip()
    if not raw.startswith(prefix):
        return None

    body = raw[len(prefix) :].strip()
    if not body:
        return None

    name, separator, args = body.partition(" ")
    del separator
    normalized = _normalize_command_name(name)
    if normalized is None:
        return None
    stripped_args = args.strip()
    return CommandInvocation(
        raw=raw,
        name=normalized,
        args=stripped_args,
        argv=tuple(stripped_args.split()),
    )


def _normalize_command_name(name: str) -> str | None:
    normalized = name.strip().lower()
    if not _COMMAND_NAME_RE.fullmatch(normalized):
        return None
    return normalized


__all__ = [
    "CommandDispatchResult",
    "CommandInvocation",
    "GatewayCommand",
    "GatewayCommandHandler",
    "GatewayCommandRegistry",
    "parse_command_text",
]
