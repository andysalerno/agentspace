"""Kernel registry — maps harness names to kernel classes."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from kernel.protocol import Kernel

from kernel_copilot import CopilotKernel
from kernel_echo import EchoKernel

KERNEL_REGISTRY: dict[str, type] = {
    "echo": EchoKernel,
    "copilot-cli": CopilotKernel,
}


def get_kernel(harness_name: str) -> Kernel:
    cls = KERNEL_REGISTRY.get(harness_name)
    if cls is None:
        available = ", ".join(sorted(KERNEL_REGISTRY.keys()))
        msg = f"Unknown kernel harness: {harness_name!r}. Available: {available}"
        raise ValueError(msg)
    return cls()  # type: ignore[return-value]
