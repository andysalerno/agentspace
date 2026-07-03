from __future__ import annotations

from dataclasses import dataclass, field

import pytest
from gateway.commands import (
    CommandInvocation,
    GatewayCommand,
    GatewayCommandRegistry,
    parse_command_text,
)


def _empty_invocations() -> list[CommandInvocation]:
    return []


@dataclass
class _CommandContext:
    invocations: list[CommandInvocation] = field(default_factory=_empty_invocations)


async def _record_invocation(
    invocation: CommandInvocation,
    context: _CommandContext,
) -> None:
    context.invocations.append(invocation)


def test_parse_command_text_normalizes_name_and_args() -> None:
    invocation = parse_command_text("  /NEW topic one  ")

    assert invocation == CommandInvocation(
        raw="/NEW topic one",
        name="new",
        args="topic one",
        argv=("topic", "one"),
    )


def test_parse_command_text_ignores_non_commands_and_invalid_names() -> None:
    assert parse_command_text("hello") is None
    assert parse_command_text("/") is None
    assert parse_command_text("/!bad") is None


@pytest.mark.asyncio
async def test_registry_dispatches_registered_command_alias() -> None:
    context = _CommandContext()
    registry = GatewayCommandRegistry(
        [
            GatewayCommand(
                name="new",
                description="Start a fresh session",
                aliases=("fresh",),
                handler=_record_invocation,
            ),
        ],
    )

    result = await registry.dispatch("/FRESH now", context)

    assert result.handled is True
    assert result.invocation is not None
    assert result.invocation.name == "fresh"
    assert context.invocations == [
        CommandInvocation(
            raw="/FRESH now",
            name="fresh",
            args="now",
            argv=("now",),
        ),
    ]


@pytest.mark.asyncio
async def test_registry_leaves_unknown_commands_unhandled() -> None:
    context = _CommandContext()
    registry = GatewayCommandRegistry(
        [
            GatewayCommand(
                name="new",
                description="Start a fresh session",
                handler=_record_invocation,
            ),
        ],
    )

    result = await registry.dispatch("/unknown", context)

    assert result.handled is False
    assert result.invocation is not None
    assert result.invocation.name == "unknown"
    assert context.invocations == []


def test_registry_rejects_duplicate_command_names_and_aliases() -> None:
    with pytest.raises(ValueError, match="duplicate"):
        GatewayCommandRegistry(
            [
                GatewayCommand(
                    name="new",
                    description="Start a fresh session",
                    aliases=("reset",),
                    handler=_record_invocation,
                ),
                GatewayCommand(
                    name="reset",
                    description="Reset session",
                    handler=_record_invocation,
                ),
            ],
        )
