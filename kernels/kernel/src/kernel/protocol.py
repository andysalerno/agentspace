"""Kernel protocol — the type contract that all kernel implementations satisfy."""

from __future__ import annotations

from collections.abc import AsyncIterator
from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable

from kernel.events import KernelEvent, KernelStatus


@dataclass(frozen=True, slots=True)
class KernelConfig:
    """Configuration passed to a kernel on start."""

    env: dict[str, str] = field(default_factory=dict)


@runtime_checkable
class Kernel(Protocol):
    """Structural type contract for a kernel implementation.

    Implementations don't need to inherit from this — they just need
    to match the shape.
    """

    @property
    def name(self) -> str: ...

    @property
    def status(self) -> KernelStatus: ...

    async def start(self, config: KernelConfig) -> None:
        """Spawn the inner harness process."""
        ...

    async def send(self, message: str) -> None:
        """Send a user message to the harness."""
        ...

    def recv(self) -> AsyncIterator[KernelEvent]:
        """Yield standard KernelEvent objects from the harness output."""
        ...

    async def stop(self) -> None:
        """Gracefully shut down the harness process."""
        ...
