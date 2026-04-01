"""Kernel registry — maps harness names to kernel classes."""

from __future__ import annotations

from enum import StrEnum
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from kernel.protocol import Kernel

from kernel_claude_code import ClaudeCodeKernel
from kernel_codex import CodexKernel
from kernel_copilot import CopilotKernel
from kernel_echo import EchoKernel


class HarnessName(StrEnum):
    CLAUDE_CODE = "claude-code"
    ECHO = "echo"
    COPILOT_CLI = "copilot-cli"
    CODEX = "codex"


KERNEL_REGISTRY: dict[HarnessName, type] = {
    HarnessName.CLAUDE_CODE: ClaudeCodeKernel,
    HarnessName.ECHO: EchoKernel,
    HarnessName.COPILOT_CLI: CopilotKernel,
    HarnessName.CODEX: CodexKernel,
}


def get_kernel(harness_name: HarnessName) -> Kernel:
    cls = KERNEL_REGISTRY.get(harness_name)
    if cls is None:
        available = ", ".join(sorted(name.value for name in KERNEL_REGISTRY))
        msg = f"Unknown kernel harness: {harness_name!r}. Available: {available}"
        raise ValueError(msg)
    return cls()  # type: ignore[return-value]
