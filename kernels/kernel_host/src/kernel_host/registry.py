"""Kernel registry — maps harness names to kernel classes."""

from __future__ import annotations

from enum import StrEnum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from kernel.protocol import Kernel

from kernel_copilot import CopilotKernel
from kernel_echo import EchoKernel


class HarnessName(StrEnum):
    ECHO = "echo"
    COPILOT_CLI = "copilot-cli"


KERNEL_REGISTRY: dict[HarnessName, type] = {
    HarnessName.ECHO: EchoKernel,
    HarnessName.COPILOT_CLI: CopilotKernel,
}


def get_kernel(harness_name: HarnessName) -> Kernel:
    cls = KERNEL_REGISTRY.get(harness_name)
    if cls is None:
        available = ", ".join(sorted(name.value for name in KERNEL_REGISTRY))
        msg = f"Unknown kernel harness: {harness_name!r}. Available: {available}"
        raise ValueError(msg)
    return cls()  # type: ignore[return-value]
